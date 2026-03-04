//! Message dispatch and domain handlers for NexusState.

use std::time::Instant;

use strata::Command;

use crate::data::Focus;

use crate::ui::context_menu::{ContextMenuItem, ContextTarget};
use crate::features::selection::drag::{ActiveKind, DragStatus, PendingIntent};
use crate::features::selection::drop as file_drop;
use crate::features::selection::snap;
use crate::features::input::SubmitRequest;
use super::message::{AnchorAction, ContextMenuMsg, DragMsg, DropZone, FileDropMsg, NexusMessage, ShellMsg, ViewerMsg};
use crate::features::selection;
use super::update_context::{UpdateContext, sync_focus_flags};
use super::NexusState;
use crate::features::shell::shell_context::build_shell_context;

// =========================================================================
// Borrow-splitting helpers
// =========================================================================

impl NexusState {
    /// Split self into (&mut ShellWidget, UpdateContext) for shell updates.
    fn shell_ctx(&mut self) -> (&mut crate::features::shell::ShellWidget, UpdateContext<'_>) {
        let ctx = UpdateContext::new(
            &mut self.scroll,
            &mut self.focus,
            &mut self.cwd,
            &mut self.context,
        );
        (&mut self.shell, ctx)
    }

    /// Split self into (&mut AgentWidget, UpdateContext) for agent updates.
    fn agent_ctx(&mut self) -> (&mut crate::features::agent::AgentWidget, UpdateContext<'_>) {
        let ctx = UpdateContext::new(
            &mut self.scroll,
            &mut self.focus,
            &mut self.cwd,
            &mut self.context,
        );
        (&mut self.agent, ctx)
    }
}

// =========================================================================
// Top-level dispatch
// =========================================================================

impl NexusState {
    /// Top-level message dispatch. Called from Component::update().
    pub(super) fn dispatch_update(
        &mut self,
        msg: NexusMessage,
        ctx: &mut strata::component::Ctx,
    ) -> Command<NexusMessage> {
        // Apply deferred scroll offset from view() (scroll-to-block)
        self.scroll.apply_pending();

        // Any interaction outside the breadcrumb cancels the disconnect prompt
        if self.disconnect_confirm.is_some() {
            let keep = matches!(
                msg,
                NexusMessage::UnnestToLevel(0)
                    | NexusMessage::DisconnectConfirmExpired
                    | NexusMessage::Tick
            );
            if !keep {
                self.disconnect_confirm = None;
            }
        }

        match msg {
            NexusMessage::Input(m) => {
                if matches!(m, super::message::InputMsg::Mouse(_)) {
                    self.set_focus(Focus::Input);
                } else {
                    self.scroll.snap_to_bottom();
                }

                // Remote tab completion: intercept and send async request
                if matches!(m, super::message::InputMsg::TabComplete) && self.remote.is_some() {
                    let remote = self.remote.as_mut().unwrap();
                    let input = self.input.text_input.text.clone();
                    let cursor = self.input.text_input.cursor;
                    self.input.completion_generation += 1;
                    let generation = self.input.completion_generation;
                    let rx = remote.complete(&input, cursor);
                    return Command::perform(async move {
                        match rx.await {
                            Ok((items, start)) => {
                                let completions = items.into_iter().map(|item| {
                                    nexus_kernel::Completion {
                                        text: item.text,
                                        display: item.display,
                                        kind: convert_completion_kind(item.kind),
                                        score: item.score,
                                    }
                                }).collect();
                                NexusMessage::Input(super::message::InputMsg::RemoteCompletionResult {
                                    completions, anchor: start, generation,
                                })
                            }
                            Err(_) => NexusMessage::Input(super::message::InputMsg::CompletionDismiss),
                        }
                    });
                }

                // Remote history search: intercept and send async request
                if let super::message::InputMsg::HistorySearchKey(ref event) = m {
                    if self.remote.is_some() {
                        // Update the query text locally
                        self.input.history_search.handle_key_local(event);
                        let query = self.input.history_search.query.clone();
                        if query.is_empty() {
                            self.input.history_search.results.clear();
                            return Command::none();
                        }
                        let remote = self.remote.as_mut().unwrap();
                        self.input.history_generation += 1;
                        let generation = self.input.history_generation;
                        let rx = remote.search_history(&query, 50);
                        return Command::perform(async move {
                            match rx.await {
                                Ok(entries) => {
                                    let results = entries.into_iter().map(|e| e.command).collect();
                                    NexusMessage::Input(super::message::InputMsg::RemoteHistoryResult {
                                        results, generation,
                                    })
                                }
                                Err(_) => NexusMessage::Input(super::message::InputMsg::HistorySearchDismiss),
                            }
                        });
                    }
                }

                let submit = self.input.update(m);
                if let Some(req) = submit {
                    self.handle_submit(req)
                } else {
                    Command::none()
                }
            }
            NexusMessage::Shell(m) => {
                // Anchor actions are cross-cutting (clipboard, spawn process)
                if let ShellMsg::OpenAnchor(_, ref action) = m {
                    self.exec_anchor_action(action);
                    return Command::none();
                }
                // Cancel in-flight remote connections on kill or interrupt
                if let ShellMsg::KillBlock(id) | ShellMsg::SendInterrupt(id) = &m {
                    if let Some(cancel) = self.connecting_tasks.remove(id) {
                        cancel.cancel();
                    }
                    // Forward cancel to remote agent
                    if let Some(ref mut remote) = self.remote {
                        remote.cancel_block(*id);
                    }
                }

                // Remote PTY input: intercept and forward to agent
                if let ShellMsg::PtyInput(block_id, ref event) = m {
                    if let Some(ref mut remote) = self.remote {
                        // If this block has no local PTY handle, it's a remote PTY
                        if !self.shell.pty.has_handle(block_id) {
                            let block = self.shell.blocks.get(block_id);
                            let flags = block
                                .map(|b| crate::features::shell::pty_backend::TermKeyFlags {
                                    app_cursor: b.parser.app_cursor(),
                                    ..Default::default()
                                })
                                .unwrap_or_default();
                            if let Some(bytes) = crate::features::shell::pty_backend::strata_key_to_bytes(&event, flags) {
                                remote.pty_input(block_id, bytes);
                            }
                            return Command::none();
                        }
                    }
                }
                // Remote connection results are handled at the root level
                let m = match m {
                    ShellMsg::RemoteConnected { block_id, remote, env } => {
                        self.handle_remote_connected(block_id, remote, env);
                        return Command::none();
                    }
                    ShellMsg::RemoteConnectFailed { block_id, error } => {
                        self.handle_remote_connect_failed(block_id, error);
                        return Command::none();
                    }
                    other => other,
                };
                let (shell, mut uctx) = self.shell_ctx();
                shell.update(m, &mut uctx, ctx.images);
                let cmds = uctx.into_commands();
                sync_focus_flags(&self.focus, &mut self.input, &mut self.agent);
                cmds
            }
            NexusMessage::Agent(m) => {
                if matches!(m, super::message::AgentMsg::QuestionInputMouse(_)) {
                    self.set_focus(Focus::AgentInput);
                }
                let (agent, mut uctx) = self.agent_ctx();
                agent.update(m, &mut uctx);
                let cmds = uctx.into_commands();
                sync_focus_flags(&self.focus, &mut self.input, &mut self.agent);
                cmds
            }
            NexusMessage::Selection(m) => {
                let snap_content = match &m {
                    super::message::SelectionMsg::Extend(addr, _)
                    | super::message::SelectionMsg::Start(addr, _, _) => {
                        self.build_snap_content(addr.source_id)
                    }
                    _ => None,
                };
                let (_cmd, _) = self.selection.update(m, ctx, snap_content.as_ref());
                Command::none()
            }
            NexusMessage::Viewer(m) => { self.dispatch_viewer_msg(m); Command::none() }
            NexusMessage::FocusBlock(id) => {
                self.set_focus(Focus::Block(id));
                Command::none()
            }
            NexusMessage::ContextMenu(m) => self.dispatch_context_menu(m),
            NexusMessage::Scroll(action) => { self.scroll.apply_user_scroll(action); Command::none() }
            NexusMessage::ScrollToJob(_) => { self.scroll.snap_to_bottom(); Command::none() }
            NexusMessage::UnnestToLevel(level) => {
                if level == 0 {
                    // Disconnect — requires confirmation (double-click within 3s)
                    if self.disconnect_confirm.is_some() {
                        // Second click — confirmed, disconnect
                        self.disconnect_confirm = None;
                        self.handle_unnest_to_level(0);
                        return Command::none();
                    }
                    // First click — enter confirmation state
                    self.disconnect_confirm = Some(Instant::now());
                    return Command::perform(async {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        NexusMessage::DisconnectConfirmExpired
                    });
                }
                self.handle_unnest_to_level(level);
                Command::none()
            }
            NexusMessage::DisconnectConfirmExpired => {
                // Clear stale confirmation (only if still pending)
                if let Some(started) = self.disconnect_confirm {
                    if started.elapsed() >= std::time::Duration::from_secs(3) {
                        self.disconnect_confirm = None;
                    }
                }
                Command::none()
            }
            NexusMessage::RemoteStateChanged(state) => {
                if let Some(ref mut remote) = self.remote {
                    remote.state = state;
                }
                Command::none()
            }
            NexusMessage::RemoteReconnected {
                request_tx,
                rtt_ms,
                last_seen_seq,
                response_rx,
                env,
            } => {
                if let Some(ref mut remote) = self.remote {
                    if let Some(rx) = response_rx.lock().unwrap().take() {
                        remote.swap_request_tx(request_tx);
                        remote.rtt_ms = rtt_ms;
                        remote.last_seen_seq = last_seen_seq;
                        remote.response_rx = rx;
                        remote.env = env;
                        remote.state = crate::features::shell::remote::ConnectionState::Connected;
                        remote.flush_queue();
                    }
                }
                Command::none()
            }
            NexusMessage::Copy => { self.copy_selection_or_input(); Command::none() }
            NexusMessage::Paste => { self.paste_from_clipboard(ctx.images); Command::none() }
            NexusMessage::ClearScreen => { self.clear_screen(); Command::none() }
            NexusMessage::CloseWindow => { self.exit_requested = true; Command::none() }
            // NewWindow and QuitApp are intercepted by the shell adapter before
            // reaching update(). If they somehow arrive here, treat as no-ops.
            NexusMessage::NewWindow | NexusMessage::QuitApp => Command::none(),
            NexusMessage::BlurAll => {
                self.transient.dismiss_all(&mut self.input);
                self.set_focus(Focus::Input);
                Command::none()
            }
            NexusMessage::Tick => {
                self.on_output_arrived();
                self.check_reconnect()
            }
            NexusMessage::FileDrop(m) => self.dispatch_file_drop(m),
            NexusMessage::Drag(m) => { self.dispatch_drag(m, ctx); Command::none() }
            NexusMessage::FocusPrevBlock => {
                let target = match self.focus {
                    Focus::Block(id) => self.prev_block_id(id),
                    Focus::Input => self.last_block_id(),
                    _ => None,
                };
                match target {
                    Some(id) => {
                        self.set_focus(Focus::Block(id));
                        self.scroll.scroll_to_block(id);
                    }
                    None => {
                        self.set_focus(Focus::Input);
                        self.scroll.snap_to_bottom();
                    }
                }
                Command::none()
            }
            NexusMessage::FocusNextBlock => {
                if let Focus::Block(id) = self.focus {
                    match self.next_block_id(id) {
                        Some(next) => {
                            self.set_focus(Focus::Block(next));
                            self.scroll.scroll_to_block(next);
                        }
                        None => {
                            self.set_focus(Focus::Input);
                            self.scroll.snap_to_bottom();
                        }
                    }
                }
                Command::none()
            }
            NexusMessage::FocusFirstBlock => {
                if let Some(id) = self.all_block_ids_ordered().first().copied() {
                    self.set_focus(Focus::Block(id));
                    self.scroll.scroll_to_block(id);
                }
                Command::none()
            }
            NexusMessage::FocusLastBlock => {
                if let Some(id) = self.last_block_id() {
                    self.set_focus(Focus::Block(id));
                    self.scroll.scroll_to_block(id);
                }
                Command::none()
            }
            NexusMessage::FocusAgentInput => {
                self.set_focus(Focus::AgentInput);
                self.scroll.snap_to_bottom();
                Command::none()
            }
            NexusMessage::TypeThrough(event) => {
                self.set_focus(Focus::Input);
                self.scroll.snap_to_bottom();
                if let Some(msg) = self.input.on_key(&event) {
                    let submit = self.input.update(msg);
                    if let Some(req) = submit {
                        self.handle_submit(req)
                    } else {
                        Command::none()
                    }
                } else {
                    Command::none()
                }
            }
            NexusMessage::ZoomIn => { self.zoom_in(); Command::none() }
            NexusMessage::ZoomOut => { self.zoom_out(); Command::none() }
            NexusMessage::ZoomReset => { self.zoom_level = 1.0; Command::none() }
            #[cfg(debug_assertions)]
            NexusMessage::ToggleDebugLayout => {
                self.debug_layout = !self.debug_layout;
                Command::none()
            }
        }
    }
}

// =========================================================================
// Cross-cutting handlers (root policy)
// =========================================================================

impl NexusState {
    fn handle_submit(&mut self, req: SubmitRequest) -> Command<NexusMessage> {
        let SubmitRequest { text, is_agent, attachments } = req;

        // Short-circuit built-in "clear" before any side effects.
        if !is_agent && text.trim() == "clear" {
            return Command::message(NexusMessage::ClearScreen);
        }

        // Handle "exit" when in remote mode — pop the backend stack.
        if !is_agent && text.trim() == "exit" && self.remote.is_some() {
            let remote = self.remote.as_mut().unwrap();
            if remote.backend_stack.is_empty() {
                // Outermost remote level — shutdown and disconnect
                let mut remote = self.remote.take().unwrap();
                remote.shutdown();
                remote.kill_child_sync();
                let cwd = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
                self.cwd = cwd;
            } else {
                // Nested level — unnest one hop, stay connected
                remote.unnest();
            }
            return Command::none();
        }

        // Append to native shell history (before execution, for crash safety).
        // Records both kernel and PTY commands.
        if !is_agent {
            self.kernel.blocking_lock().append_history(&text);
        }

        self.input.reset_history_nav();

        if is_agent {
            let block_id = self.next_id();
            let contextualized_query = if self.agent.session_id.is_some() {
                format!("[CWD: {}]\n{}", self.cwd, text)
            } else {
                let shell_context = build_shell_context(
                    &self.cwd,
                    &self.shell.blocks.blocks,
                    self.input.shell_history(),
                );
                format!("{}{}", shell_context, text)
            };
            self.agent.spawn(block_id, text, contextualized_query, attachments, &self.cwd);
            self.scroll.snap_to_bottom();
        } else {
            let block_id = self.next_id();
            let kernel = self.kernel.clone();
            let kernel_tx = self.kernel_tx.clone();
            let cwd = self.cwd.clone();
            // Manual borrow splitting: shell_ctx() borrows scroll/focus/cwd/context,
            // but we also need &mut remote which is a separate field.
            let uctx = UpdateContext::new(
                &mut self.scroll,
                &mut self.focus,
                &mut self.cwd,
                &mut self.context,
            );
            let shell = &mut self.shell;
            let remote = &mut self.remote;
            let mut uctx = uctx;
            let remote_transport = shell.execute(
                text,
                block_id,
                &cwd,
                &kernel,
                &kernel_tx,
                remote.as_mut(),
                &mut uctx,
            );
            let cmds = uctx.into_commands();
            sync_focus_flags(&self.focus, &mut self.input, &mut self.agent);

            if let Some(ssh_command) = remote_transport {
                // Already connected — nest via the existing connection
                if let Some(ref mut remote) = self.remote {
                    if let Some(transport) = parse_remote_command(&ssh_command) {
                        remote.nest(transport);
                    }
                    return cmds;
                }

                // Not connected — first connection, spawn async task
                let kernel_tx = self.kernel_tx.clone();
                let cancel = tokio_util::sync::CancellationToken::new();
                self.connecting_tasks.insert(block_id, cancel.clone());
                let connect_cmd = Command::perform(async move {
                    match Self::connect_remote(ssh_command, kernel_tx, block_id, cancel).await {
                        Ok((remote, env)) => NexusMessage::Shell(ShellMsg::RemoteConnected {
                            block_id,
                            remote: std::sync::Arc::new(std::sync::Mutex::new(Some(remote))),
                            env: Box::new(env),
                        }),
                        Err(e) => NexusMessage::Shell(ShellMsg::RemoteConnectFailed {
                            block_id,
                            error: e.to_string(),
                        }),
                    }
                });
                return Command::batch(vec![cmds, connect_cmd]);
            }

            return cmds;
        }

        Command::none()
    }

    /// Async task to establish a remote connection.
    async fn connect_remote(
        command: String,
        kernel_tx: tokio::sync::broadcast::Sender<nexus_api::ShellEvent>,
        block_id: nexus_api::BlockId,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<(
        crate::features::shell::remote::RemoteBackend,
        nexus_protocol::messages::EnvInfo,
    )> {
        tokio::select! {
            result = Self::connect_remote_inner(command, kernel_tx, block_id) => result,
            _ = cancel.cancelled() => Err(anyhow::anyhow!("connection cancelled")),
        }
    }

    /// Inner logic for remote connection (separated for cancellation wrapping).
    async fn connect_remote_inner(
        command: String,
        kernel_tx: tokio::sync::broadcast::Sender<nexus_api::ShellEvent>,
        block_id: nexus_api::BlockId,
    ) -> anyhow::Result<(
        crate::features::shell::remote::RemoteBackend,
        nexus_protocol::messages::EnvInfo,
    )> {
        use crate::features::shell::remote::deploy::ProgressReporter;

        let progress = ProgressReporter::new(kernel_tx.clone(), block_id);

        let transport = parse_remote_command(&command)
            .ok_or_else(|| anyhow::anyhow!("could not parse remote transport: {command}"))?;

        // For SSH transports, deploy the agent binary first
        let agent_path = match &transport {
            nexus_protocol::messages::Transport::Ssh {
                destination,
                port,
                identity,
                extra_args,
            } => {
                crate::features::shell::remote::deploy::ensure_deployed(
                    destination,
                    *port,
                    identity.as_deref(),
                    extra_args,
                    progress.clone(),
                )
                .await?
            }
            // Docker/kubectl: agent must already be at /tmp/nexus-agent
            // (or deployed separately). Use a well-known path.
            _ => {
                progress.emit("Connecting...", None, None);
                format!("~/.nexus/{}", crate::features::shell::remote::deploy::agent_binary_name())
            }
        };

        progress.emit("Connecting...", None, None);
        let forwarded_env = collect_forwarded_env();
        let (handle, env, _session_token, request_tx) =
            crate::features::shell::remote::transport::TransportHandle::connect(
                &transport,
                &agent_path,
                forwarded_env,
                kernel_tx.clone(),
            )
            .await?;

        progress.emit("Connected", Some(&format!("{}@{}", env.user, env.hostname)), Some(1.0));

        // Emit a synthetic CommandFinished for the connection block
        let _ = kernel_tx.send(nexus_api::ShellEvent::CommandFinished {
            block_id,
            exit_code: 0,
            duration_ms: 0,
        });

        let remote = crate::features::shell::remote::RemoteBackend::new(
            env.clone(),
            request_tx,
            handle.rtt_ms,
            handle.last_seen_seq,
            handle.response_rx,
            Some(handle.child),
        );

        Ok((remote, env))
    }

    /// Handle a successful remote connection.
    fn handle_remote_connected(
        &mut self,
        block_id: nexus_api::BlockId,
        remote: std::sync::Arc<std::sync::Mutex<Option<crate::features::shell::remote::RemoteBackend>>>,
        env: Box<nexus_protocol::messages::EnvInfo>,
    ) {
        self.connecting_tasks.remove(&block_id);
        // Take the remote backend out of the Arc<Mutex<Option<...>>>
        let remote = remote.lock().unwrap().take();
        if let Some(remote) = remote {
            // Update CWD to remote CWD
            self.cwd = env.cwd.display().to_string();
            self.remote = Some(remote);
            tracing::info!(
                "connected to remote: {}@{} ({})",
                env.user,
                env.hostname,
                env.cwd.display()
            );
        }
    }

    /// Check if we need to start a reconnection attempt.
    /// Called on each Tick. Only triggers once (state transition Disconnected → Reconnecting).
    fn check_reconnect(&mut self) -> Command<NexusMessage> {
        use crate::features::shell::remote::ConnectionState;

        // Detect SSH child death → mark as Disconnected
        if let Some(ref mut remote) = self.remote {
            if remote.state == ConnectionState::Connected && !remote.check_child_alive() {
                tracing::warn!("SSH child process died");
                remote.state = ConnectionState::Disconnected;
            }
        }

        let should_reconnect = self
            .remote
            .as_ref()
            .map_or(false, |r| r.state == ConnectionState::Disconnected);

        if !should_reconnect {
            return Command::none();
        }

        // Transition to Reconnecting so we don't spawn multiple tasks
        if let Some(ref mut remote) = self.remote {
            remote.state = ConnectionState::Reconnecting;
        }

        // We need the SSH command to reconnect. For now, we don't store the original
        // SSH command, so reconnection would need the environment info.
        // Since the transport handle doesn't persist the destination, we can't
        // reconnect without it. This is a limitation — for a full implementation,
        // we'd store the SSH destination in RemoteBackend.
        //
        // For now, just set the state and let the user reconnect manually or
        // use UnnestToLevel(0) to disconnect.
        tracing::info!("remote connection lost, state set to Reconnecting");
        Command::none()
    }

    /// Handle breadcrumb click: unnest to the specified depth.
    fn handle_unnest_to_level(&mut self, level: usize) {
        if level == 0 {
            // Disconnect entirely
            self.disconnect_confirm = None;
            if let Some(mut remote) = self.remote.take() {
                remote.shutdown();
                remote.kill_child_sync();
                let cwd = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
                self.cwd = cwd;
            }
        } else if let Some(ref mut remote) = self.remote {
            // Pop back to the target level by sending Unnest for each extra level
            let current_depth = remote.backend_stack.len();
            for _ in level..current_depth {
                remote.unnest();
            }
        }
    }

    /// Handle a failed remote connection.
    fn handle_remote_connect_failed(
        &mut self,
        block_id: nexus_api::BlockId,
        error: String,
    ) {
        self.connecting_tasks.remove(&block_id);
        // Emit error as stderr on the block
        let _ = self.kernel_tx.send(nexus_api::ShellEvent::StderrChunk {
            block_id,
            data: format!("Remote connection failed: {}\n", error).into_bytes(),
        });
        let _ = self.kernel_tx.send(nexus_api::ShellEvent::CommandFinished {
            block_id,
            exit_code: 1,
            duration_ms: 0,
        });
    }

    fn dispatch_drag(&mut self, msg: DragMsg, ctx: &mut strata::component::Ctx) {
        match msg {
            DragMsg::Start(intent, origin) => {
                self.drag.status = DragStatus::Pending {
                    origin,
                    intent,
                };
            }
            DragMsg::StartSelecting(addr, mode, position) => {
                // If the click landed on a shell block, focus it so keyboard
                // input flows to its PTY.
                if let Some(block_id) = self.shell.block_for_source(addr.source_id) {
                    self.set_focus(crate::data::Focus::Block(block_id));
                }
                // Immediate Active — no Pending hysteresis for raw text clicks.
                let snap_content = self.build_snap_content(addr.source_id);
                self.selection.update(
                    super::message::SelectionMsg::Start(addr.clone(), mode, position),
                    ctx,
                    snap_content.as_ref(),
                );
                self.drag.status = DragStatus::Active(ActiveKind::Selecting);
            }
            DragMsg::Activate(_position) => {
                if let DragStatus::Pending { intent, .. } =
                    std::mem::replace(&mut self.drag.status, DragStatus::Inactive)
                {
                    let drag_source = match intent {
                        PendingIntent::Anchor { payload, .. } => {
                            Some(self.payload_to_drag_source(&payload))
                        }
                        PendingIntent::SelectionDrag { text, .. } => {
                            Some(strata::DragSource::Text(text))
                        }
                        PendingIntent::RowDrag { block_id, row_index, .. } => {
                            let payload = crate::features::selection::drag::DragPayload::TableRow {
                                block_id,
                                row_index,
                                display: self.shell.row_display_text(block_id, row_index),
                            };
                            Some(self.payload_to_drag_source(&payload))
                        }
                        // ColumnResize, ColumnReorder, TerminalCapture — not drag sources
                        _ => None,
                    };
                    if let Some(source) = drag_source {
                        if let Err(e) = strata::platform::start_drag(&source) {
                            tracing::warn!("Native drag failed: {}", e);
                        }
                    }
                }
            }
            DragMsg::Cancel => {
                let prev = std::mem::replace(&mut self.drag.status, DragStatus::Inactive);
                match prev {
                    DragStatus::Pending { intent, .. } => {
                        match intent {
                            PendingIntent::Anchor { source, source_rect, .. } => {
                                // Re-dispatch click to anchor handler
                                if let Some(msg) = self.shell.on_click_anchor(source) {
                                    if let ShellMsg::OpenAnchor(_, ref action) = msg {
                                        self.exec_anchor_action_with_rect(action, source_rect);
                                    }
                                }
                            }
                            PendingIntent::SelectionDrag { origin_addr, .. } => {
                                // Click inside selection without drag → clear selection, place caret
                                self.selection.update(
                                    super::message::SelectionMsg::Start(origin_addr, crate::features::selection::drag::SelectMode::Char, strata::primitives::Point::ORIGIN),
                                    ctx,
                                    None,
                                );
                            }
                            // Future intents — no-op
                            _ => {}
                        }
                    }
                    DragStatus::Active(ActiveKind::Selecting { .. }) => {
                        // Selection ended (mouse released or cursor left)
                        self.selection.update(
                            super::message::SelectionMsg::End,
                            ctx,
                            None,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// Convert a drag payload to a native drag source, using shell state for block lookups.
    fn payload_to_drag_source(&self, payload: &crate::features::selection::drag::DragPayload) -> strata::DragSource {
        use crate::features::selection::drag::BlockSnapshot;

        payload.to_drag_source(|block_id| {
            self.shell
                .block_by_id(block_id)
                .map(BlockSnapshot::from_block)
        })
    }

    /// Build a snap content snapshot for the given source ID.
    pub(crate) fn build_snap_content(&self, source_id: strata::content_address::SourceId) -> Option<snap::SnapContent> {
        use crate::utils::ids as source_ids;

        let text_snap = |text: String| -> snap::SnapContent {
            snap::SnapContent::Text { lines: text.lines().map(String::from).collect() }
        };

        // Check shell blocks
        for block in &self.shell.blocks.blocks {
            if source_id == source_ids::shell_term(block.id) && block.structured_output.is_none() {
                let grid = if block.parser.is_alternate_screen() {
                    block.parser.grid()
                } else {
                    block.parser.grid_with_scrollback()
                };
                let chars = grid.cells().iter().map(|c| c.c).collect();
                return Some(snap::SnapContent::Grid { chars, cols: grid.cols() as usize });
            }
            if source_id == source_ids::shell_header(block.id) {
                return Some(text_snap(format!("$ {}", block.command)));
            }
            if source_id == source_ids::native(block.id) {
                if let Some(ref value) = block.structured_output {
                    return Some(text_snap(value.to_text()));
                }
            }
            if source_id == source_ids::table(block.id) {
                if let Some(nexus_api::Value::Table { columns, rows }) = &block.structured_output {
                    // Build lines matching the table's register_source order:
                    // data cells only (row-by-row, column-by-column). Headers are
                    // not registered as source items — they're display/sort-only.
                    let mut lines = Vec::with_capacity(rows.len() * columns.len());
                    for row in rows {
                        for (col_idx, cell) in row.iter().enumerate() {
                            let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                                nexus_api::format_value_for_display(cell, fmt)
                            } else {
                                cell.to_text()
                            };
                            lines.push(text);
                        }
                    }
                    return Some(snap::SnapContent::Text { lines });
                }
            }
        }
        // Check agent blocks
        for block in &self.agent.blocks {
            if source_id == source_ids::agent_response(block.id) {
                return Some(text_snap(block.response.clone()));
            }
            if source_id == source_ids::agent_thinking(block.id) {
                return Some(text_snap(block.thinking.clone()));
            }
            if source_id == source_ids::agent_query(block.id) {
                return Some(snap::SnapContent::Text {
                    lines: vec!["?".to_string(), block.query.clone()],
                });
            }
            for (i, tool) in block.tools.iter().enumerate() {
                if source_id == source_ids::agent_tool(block.id, i) {
                    return Some(text_snap(tool.extract_text()));
                }
            }
            if let Some(ref perm) = block.pending_permission {
                if source_id == source_ids::agent_perm_text(block.id) {
                    let mut text = String::from("\u{26A0} Permission Required\n");
                    text.push_str(&perm.description);
                    text.push('\n');
                    text.push_str(&perm.action);
                    if let Some(ref dir) = perm.working_dir {
                        text.push_str(&format!("\nin {}", dir));
                    }
                    return Some(text_snap(text));
                }
            }
            if let Some(ref q) = block.pending_question {
                if source_id == source_ids::agent_question_text(block.id) {
                    let mut text = String::from("\u{2753} Claude is asking:\n");
                    for question in &q.questions {
                        text.push_str(&question.question);
                        text.push('\n');
                    }
                    return Some(text_snap(text));
                }
            }
            if source_id == source_ids::agent_footer(block.id) {
                return Some(text_snap(block.footer_text()));
            }
        }
        None
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        if !self.input.text_input.text.is_empty()
            && !self.input.text_input.text.ends_with(' ')
        {
            self.input.text_input.text.push(' ');
        }
        self.input.text_input.text.push_str(text);
        self.input.text_input.cursor = self.input.text_input.text.len();
    }

    fn dispatch_file_drop(&mut self, msg: FileDropMsg) -> Command<NexusMessage> {
        match msg {
            FileDropMsg::Hovered(_path, zone) => {
                self.drop_highlight = Some(zone);
                Command::none()
            }
            FileDropMsg::Dropped(path, zone) => {
                self.drop_highlight = None;
                // Check if this is our own drag data coming back via native round-trip
                if let Some(text) = file_drop::read_temp_file_content(&path) {
                    self.insert_text_at_cursor(&text);
                    return Command::none();
                }
                match zone {
                    DropZone::InputBar | DropZone::Empty => {
                        let quoted = file_drop::shell_quote(&path);
                        self.insert_text_at_cursor(&quoted);
                        Command::none()
                    }
                    DropZone::AgentPanel => {
                        // Async read — don't block the UI thread
                        let path_clone = path.clone();
                        Command::perform(async move {
                            match tokio::fs::read(&path_clone).await {
                                Ok(data) => {
                                    if data.len() > 10 * 1024 * 1024 {
                                        NexusMessage::FileDrop(FileDropMsg::FileLoadFailed(
                                            path_clone,
                                            "File exceeds 10 MB limit".into(),
                                        ))
                                    } else {
                                        NexusMessage::FileDrop(FileDropMsg::FileLoaded(path_clone, data))
                                    }
                                }
                                Err(e) => NexusMessage::FileDrop(FileDropMsg::FileLoadFailed(
                                    path_clone,
                                    e.to_string(),
                                )),
                            }
                        })
                    }
                    DropZone::ShellBlock(_) => {
                        let quoted = file_drop::shell_quote(&path);
                        self.insert_text_at_cursor(&quoted);
                        Command::none()
                    }
                }
            }
            FileDropMsg::HoverLeft => {
                self.drop_highlight = None;
                Command::none()
            }
            FileDropMsg::FileLoaded(path, data) => {
                // Create an attachment from the loaded file data
                // For now, insert the path into the input as context
                let filename = path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                tracing::info!("File loaded for agent: {} ({} bytes)", filename, data.len());
                // TODO: Create proper attachment when agent attachment API is ready
                let quoted = file_drop::shell_quote(&path);
                self.insert_text_at_cursor(&quoted);
                Command::none()
            }
            FileDropMsg::FileLoadFailed(path, reason) => {
                tracing::warn!("File drop failed for {}: {}", path.display(), reason);
                Command::none()
            }
        }
    }

    fn exec_anchor_action(&self, action: &AnchorAction) {
        self.exec_anchor_action_with_rect(action, None);
    }

    fn exec_anchor_action_with_rect(&self, action: &AnchorAction, source_rect: Option<strata::primitives::Rect>) {
        match action {
            AnchorAction::QuickLook(path) => {
                // Preview with native Quick Look (macOS)
                let result = if let Some(local_rect) = source_rect {
                    // Use local rect for zoom animation
                    strata::platform::preview_file_with_local_rect(path, local_rect)
                } else {
                    // No animation
                    strata::platform::preview_file(path)
                };
                if let Err(e) = result {
                    tracing::warn!("Quick Look failed: {}", e);
                }
            }
            AnchorAction::RevealPath(path) => {
                // Reveal in Finder (macOS) — `open -R <path>`
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(path)
                    .spawn();
            }
            AnchorAction::Open(path) => {
                // Open with default application (macOS) — `open <path>`
                let _ = std::process::Command::new("open")
                    .arg(path)
                    .spawn();
            }
            AnchorAction::OpenUrl(url) => {
                let _ = std::process::Command::new("open")
                    .arg(url)
                    .spawn();
            }
            AnchorAction::CopyToClipboard(text) => {
                Self::set_clipboard_text(text);
            }
        }
    }

    fn copy_selection_or_input(&mut self) {
        // Try content selection first
        if let Some(text) =
            self.selection
                .extract_selected_text(&self.shell.blocks.blocks, &self.agent.blocks)
        {
            Self::set_clipboard_text(&text);
            return;
        }

        // Fall back to input text selection
        if let Some((sel_start, sel_end)) = self.input.text_input.selection {
            let start = sel_start.min(sel_end);
            let end = sel_start.max(sel_end);
            if start != end {
                let selected: String = self.input.text_input.text
                    .chars()
                    .skip(start)
                    .take(end - start)
                    .collect();
                if !selected.is_empty() {
                    Self::set_clipboard_text(&selected);
                }
            }
        }
    }

    fn dispatch_context_menu(&mut self, msg: ContextMenuMsg) -> Command<NexusMessage> {
        match msg {
            ContextMenuMsg::Show(x, y, items, target) => {
                // Build native menu items
                let native_items: Vec<strata::platform::NativeMenuItem> = items.iter().map(|item| {
                    strata::platform::NativeMenuItem {
                        label: item.label().to_string(),
                        shortcut: String::new(),
                        separator: false,
                    }
                }).collect();

                // Convert from layout space (zoomed) to view coordinates
                let z = self.zoom_level;
                let vx = x * z;
                let vy = y * z;

                // Store items + target for on_native_menu_result callback
                self.pending_menu_items = Some((items, target));

                // Defer the native menu to show after the state borrow is released
                // (NSMenu blocks the run loop, causing re-entrant borrow panics)
                strata::platform::request_native_menu(native_items, vx, vy);
                Command::none()
            }
            ContextMenuMsg::Action(item) => self.exec_context_menu_item(item, None),
            ContextMenuMsg::ActionWithTarget(item, target) => self.exec_context_menu_item(item, Some(target)),
            ContextMenuMsg::Dismiss => Command::none(),
        }
    }

    fn exec_context_menu_item(&mut self, item: ContextMenuItem, target: Option<ContextTarget>) -> Command<NexusMessage> {
        match item {
            ContextMenuItem::Copy => {
                // First try to copy the selected text (respects user's selection)
                if let Some(text) = self
                    .selection
                    .extract_selected_text(&self.shell.blocks.blocks, &self.agent.blocks)
                {
                    Self::set_clipboard_text(&text);
                    return Command::none();
                }
                // Fall back to input selection if in input context
                if matches!(target, Some(ContextTarget::Input)) {
                    if let Some((sel_start, sel_end)) = self.input.text_input.selection {
                        let start = sel_start.min(sel_end);
                        let end = sel_start.max(sel_end);
                        if start != end {
                            let selected: String = self.input.text_input.text
                                .chars()
                                .skip(start)
                                .take(end - start)
                                .collect();
                            if !selected.is_empty() {
                                Self::set_clipboard_text(&selected);
                                return Command::none();
                            }
                        }
                    }
                }
                // Fall back to entire block text only if no selection
                if let Some(text) = target.and_then(|t| {
                    selection::extract_block_text(
                        &self.shell.blocks,
                        &self.agent.blocks,
                        &self.agent.block_index,
                        &self.input.text_input.text,
                        &t,
                    )
                }) {
                    Self::set_clipboard_text(&text);
                }
            }
            ContextMenuItem::Paste => {
                return Command::message(NexusMessage::Paste);
            }
            ContextMenuItem::SelectAll => match target.as_ref() {
                Some(ContextTarget::Input) | None => {
                    self.input.text_input.select_all();
                }
                Some(ContextTarget::Block(_)) | Some(ContextTarget::AgentBlock(_))
                | Some(ContextTarget::TableCell { .. }) => {
                    self.selection
                        .select_all(&self.shell.blocks.blocks, &self.agent.blocks);
                }
            },
            ContextMenuItem::Clear => {
                self.input.text_input.text.clear();
                self.input.text_input.cursor = 0;
                self.input.text_input.selection = None;
            }
            ContextMenuItem::CopyCommand => {
                if let Some(block) = self.target_shell_block(&target) {
                    Self::set_clipboard_text(&block.command);
                }
            }
            ContextMenuItem::CopyOutput => {
                if let Some(block) = self.target_shell_block(&target) {
                    Self::set_clipboard_text(&block.copy_output());
                }
            }
            ContextMenuItem::CopyAsTsv => {
                if let Some(block) = self.target_shell_block(&target) {
                    if let Some(tsv) = block.copy_as_tsv() {
                        Self::set_clipboard_text(&tsv);
                    }
                }
            }
            ContextMenuItem::CopyAsJson => {
                if let Some(block) = self.target_shell_block(&target) {
                    if let Some(json) = block.copy_as_json() {
                        Self::set_clipboard_text(&json);
                    }
                }
            }
            ContextMenuItem::Rerun => {
                if let Some(block) = self.target_shell_block(&target) {
                    let cmd = block.command.clone();
                    return self.handle_submit(SubmitRequest {
                        text: cmd,
                        is_agent: false,
                        attachments: Vec::new(),
                    });
                }
            }
            ContextMenuItem::QuickLook(path) => {
                if let Err(e) = strata::platform::preview_file(&path) {
                    tracing::warn!("Quick Look failed: {}", e);
                }
            }
            ContextMenuItem::Open(path) => {
                let _ = std::process::Command::new("open")
                    .arg(&path)
                    .spawn();
            }
            ContextMenuItem::CopyPath(path) => {
                Self::set_clipboard_text(&path.display().to_string());
            }
            ContextMenuItem::RevealInFinder(path) => {
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn();
            }
            ContextMenuItem::CopyCellValue(text) => {
                Self::set_clipboard_text(&text);
            }
            ContextMenuItem::FilterByValue { value, col } => {
                if let Some(block_id) = self.target_shell_block_id(&target) {
                    return Command::message(NexusMessage::Shell(
                        ShellMsg::FilterTable(block_id, col, Some(crate::data::ColumnFilter::Equals(value)))
                    ));
                }
            }
            ContextMenuItem::ExcludeValue { value, col } => {
                if let Some(block_id) = self.target_shell_block_id(&target) {
                    return Command::message(NexusMessage::Shell(
                        ShellMsg::FilterTable(block_id, col, Some(crate::data::ColumnFilter::NotEquals(value)))
                    ));
                }
            }
            ContextMenuItem::ClearColumnFilter(block_id, col) => {
                return Command::message(NexusMessage::Shell(
                    ShellMsg::FilterTable(block_id, col, None)
                ));
            }
            ContextMenuItem::ClearAllFilters(block_id) => {
                return Command::message(NexusMessage::Shell(
                    ShellMsg::ClearAllFilters(block_id)
                ));
            }
        }
        Command::none()
    }

    /// Resolve a context target to the shell block it refers to.
    fn target_shell_block<'a>(&'a self, target: &Option<ContextTarget>) -> Option<&'a crate::data::Block> {
        match target {
            Some(ContextTarget::Block(id)) | Some(ContextTarget::TableCell { block_id: id, .. }) => {
                self.shell.blocks.get(*id)
            }
            _ => None,
        }
    }

    /// Resolve a context target to its block ID.
    fn target_shell_block_id(&self, target: &Option<ContextTarget>) -> Option<nexus_api::BlockId> {
        match target {
            Some(ContextTarget::Block(id)) | Some(ContextTarget::TableCell { block_id: id, .. }) => Some(*id),
            _ => None,
        }
    }
}

// =========================================================================
// Viewer message handler
// =========================================================================

impl NexusState {
    /// Dispatch a viewer message to the appropriate block.
    /// Viewer logic is encapsulated in Block::update_viewer().
    fn dispatch_viewer_msg(&mut self, msg: ViewerMsg) {
        // Exit is special — has side effects beyond block state
        if let ViewerMsg::Exit(id) = msg {
            // Cancel directly via the free function — does NOT require the kernel
            // mutex, which may be held by the command's blocking loop (e.g. top).
            nexus_kernel::commands::cancel_block(id);
            if let Some(block) = self.shell.block_by_id_mut(id) {
                block.view_state = None;
                block.version += 1;
            }
            self.set_focus(Focus::Input);
            return;
        }

        // All other messages delegate to Block::update_viewer()
        let block_id = msg.block_id();
        if let Some(block) = self.shell.block_by_id_mut(block_id) {
            block.update_viewer(&msg);
        }
    }
}

// =========================================================================
// Remote command parsing and env forwarding
// =========================================================================

/// Parse a remote transport command into its protocol `Transport` enum.
fn parse_remote_command(command: &str) -> Option<nexus_protocol::messages::Transport> {
    let mut words = command.split_whitespace();
    let first = words.next()?;

    match first {
        "ssh" => Some(parse_ssh_transport(words)),
        "docker" => parse_docker_transport(words),
        "kubectl" => parse_kubectl_transport(words),
        _ => None,
    }
}

/// Parse `ssh [options] destination` into `Transport::Ssh`.
fn parse_ssh_transport<'a>(
    words: impl Iterator<Item = &'a str>,
) -> nexus_protocol::messages::Transport {
    let mut port: Option<u16> = None;
    let mut identity: Option<String> = None;
    let mut destination = String::new();
    let mut extra_args: Vec<String> = Vec::new();
    let mut next_is_port = false;
    let mut next_is_identity = false;
    let mut skip_next = false;

    for word in words {
        if skip_next {
            extra_args.push(word.to_string());
            skip_next = false;
            continue;
        }
        if next_is_port {
            port = word.parse().ok();
            next_is_port = false;
            continue;
        }
        if next_is_identity {
            identity = Some(word.to_string());
            next_is_identity = false;
            continue;
        }

        if word == "-p" {
            next_is_port = true;
            continue;
        }
        if word == "-i" {
            next_is_identity = true;
            continue;
        }
        if word.starts_with('-') {
            let takes_value = matches!(
                word,
                "-l" | "-o" | "-F" | "-J" | "-W" | "-b" | "-c" | "-D"
                    | "-e" | "-I" | "-L" | "-m" | "-O" | "-Q" | "-R"
                    | "-S" | "-w"
            );
            extra_args.push(word.to_string());
            if takes_value {
                skip_next = true;
            }
            continue;
        }

        // First non-option argument is the destination
        if destination.is_empty() {
            destination = word.to_string();
        }
        // Continue parsing — SSH allows options after the destination
    }

    nexus_protocol::messages::Transport::Ssh {
        destination,
        port,
        identity,
        extra_args,
    }
}

/// Parse `docker exec [-u user] container` into `Transport::Docker`.
fn parse_docker_transport<'a>(
    words: impl Iterator<Item = &'a str>,
) -> Option<nexus_protocol::messages::Transport> {
    let words: Vec<&str> = words.collect();

    // Must have "exec" as first subcommand
    if words.first().copied() != Some("exec") {
        return None;
    }

    let mut user: Option<String> = None;
    let mut container = String::new();
    let mut i = 1; // skip "exec"

    while i < words.len() {
        let word = words[i];
        if word == "-u" || word == "--user" {
            if i + 1 < words.len() {
                user = Some(words[i + 1].to_string());
                i += 2;
                continue;
            }
        }
        // Skip common flags
        if word.starts_with('-') {
            i += 1;
            continue;
        }
        // First non-flag is the container name
        if container.is_empty() {
            container = word.to_string();
        }
        i += 1;
    }

    if container.is_empty() {
        return None;
    }

    Some(nexus_protocol::messages::Transport::Docker { container, user })
}

/// Parse `kubectl exec [-n ns] [-c container] pod` into `Transport::Kubectl`.
fn parse_kubectl_transport<'a>(
    words: impl Iterator<Item = &'a str>,
) -> Option<nexus_protocol::messages::Transport> {
    let words: Vec<&str> = words.collect();

    if words.first().copied() != Some("exec") {
        return None;
    }

    let mut namespace: Option<String> = None;
    let mut container: Option<String> = None;
    let mut pod = String::new();
    let mut i = 1;

    while i < words.len() {
        let word = words[i];
        if word == "-n" || word == "--namespace" {
            if i + 1 < words.len() {
                namespace = Some(words[i + 1].to_string());
                i += 2;
                continue;
            }
        }
        if word == "-c" || word == "--container" {
            if i + 1 < words.len() {
                container = Some(words[i + 1].to_string());
                i += 2;
                continue;
            }
        }
        if word == "--" {
            break;
        }
        if word.starts_with('-') {
            i += 1;
            continue;
        }
        if pod.is_empty() {
            pod = word.to_string();
        }
        i += 1;
    }

    if pod.is_empty() {
        return None;
    }

    Some(nexus_protocol::messages::Transport::Kubectl {
        pod,
        namespace,
        container,
    })
}

/// Convert protocol CompletionKind to kernel CompletionKind.
fn convert_completion_kind(kind: nexus_protocol::messages::CompletionKind) -> nexus_kernel::CompletionKind {
    match kind {
        nexus_protocol::messages::CompletionKind::File => nexus_kernel::CompletionKind::File,
        nexus_protocol::messages::CompletionKind::Directory => nexus_kernel::CompletionKind::Directory,
        nexus_protocol::messages::CompletionKind::Executable => nexus_kernel::CompletionKind::Executable,
        nexus_protocol::messages::CompletionKind::Builtin => nexus_kernel::CompletionKind::Builtin,
        nexus_protocol::messages::CompletionKind::NativeCommand => nexus_kernel::CompletionKind::NativeCommand,
        nexus_protocol::messages::CompletionKind::Function => nexus_kernel::CompletionKind::Function,
        nexus_protocol::messages::CompletionKind::Alias => nexus_kernel::CompletionKind::Alias,
        nexus_protocol::messages::CompletionKind::Variable => nexus_kernel::CompletionKind::Variable,
        nexus_protocol::messages::CompletionKind::GitBranch => nexus_kernel::CompletionKind::GitBranch,
        nexus_protocol::messages::CompletionKind::Flag => nexus_kernel::CompletionKind::Flag,
    }
}

/// Collect environment variables to forward to the remote agent.
fn collect_forwarded_env() -> std::collections::HashMap<String, String> {
    let keys = [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "EDITOR",
        "VISUAL",
        "LANG",
    ];

    let mut env = std::collections::HashMap::new();
    for key in &keys {
        if let Ok(val) = std::env::var(key) {
            env.insert(key.to_string(), val);
        }
    }

    // Forward LC_* variables
    for (key, val) in std::env::vars() {
        if key.starts_with("LC_") {
            env.insert(key, val);
        }
    }

    env
}
