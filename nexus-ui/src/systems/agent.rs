//! Agent system for AI assistant integration.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use iced::futures::stream;
use iced::Subscription;
use tokio::sync::{mpsc, Mutex};

use crate::agent_adapter::{AgentEvent, IcedAgentUI};

/// No-op persistence for agent state (we don't persist agent sessions yet).
pub struct NoopPersistence;

impl nexus_agent::agent::persistence::AgentStatePersistence for NoopPersistence {
    fn save_agent_state(&mut self, _state: nexus_agent::SessionState) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Spawn an agent task to process a query.
pub async fn spawn_agent_task(
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    cancel_flag: Arc<AtomicBool>,
    query: String,
    working_dir: PathBuf,
) -> anyhow::Result<()> {
    use nexus_agent::{Agent, AgentComponents, SessionConfig};
    use nexus_executor::DefaultCommandExecutor;
    use nexus_llm::factory::create_llm_client_from_model;

    // Try to detect which model to use based on environment
    let model_name = if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        "claude-sonnet"
    } else if std::env::var("OPENAI_API_KEY").is_ok() {
        "gpt-4o"
    } else {
        // Send error event if no API key is configured
        let _ = event_tx.send(AgentEvent::Error(
            "No API key found. Set ANTHROPIC_API_KEY or OPENAI_API_KEY environment variable."
                .to_string(),
        ));
        return Ok(());
    };

    tracing::info!("Creating LLM client for model: {}", model_name);

    // Create LLM provider
    let llm_provider = match create_llm_client_from_model(model_name, None, false, None).await {
        Ok(provider) => provider,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Error(format!(
                "Failed to create LLM client: {}",
                e
            )));
            return Ok(());
        }
    };

    // Create components with cancel flag connected to UI
    let ui = Arc::new(IcedAgentUI::with_cancel_flag(event_tx.clone(), cancel_flag));

    let components = AgentComponents {
        llm_provider,
        project_manager: Box::new(nexus_agent::config::DefaultProjectManager::new()),
        command_executor: Box::new(DefaultCommandExecutor),
        ui,
        state_persistence: Box::new(NoopPersistence),
        permission_handler: None, // TODO: Add permission handling
        sub_agent_runner: None,
    };

    // Create session config
    let session_config = SessionConfig {
        init_path: Some(working_dir),
        ..Default::default()
    };

    // Create and run agent
    let mut agent = Agent::new(components, session_config);

    // Initialize project context
    if let Err(e) = agent.init_project_context() {
        let _ = event_tx.send(AgentEvent::Error(format!(
            "Failed to init project context: {}",
            e
        )));
        return Ok(());
    }

    // Add the user message
    if let Err(e) = agent.append_message(nexus_llm::Message::new_user(query)) {
        let _ = event_tx.send(AgentEvent::Error(format!("Failed to add message: {}", e)));
        return Ok(());
    }

    // Run the agent iteration
    if let Err(e) = agent.run_single_iteration().await {
        let _ = event_tx.send(AgentEvent::Error(format!("Agent error: {}", e)));
    }

    Ok(())
}

/// Async subscription that awaits agent events.
/// Returns raw AgentEvent for caller to map to messages.
pub fn agent_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
) -> Subscription<AgentEvent> {
    struct AgentSubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<AgentSubscription>(),
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };

            event.map(|agent_event| (agent_event, rx))
        }),
    )
}
