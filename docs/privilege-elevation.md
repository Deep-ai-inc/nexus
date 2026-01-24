# Privilege Elevation in Nexus

Nexus uses native OS authentication for privilege elevation rather than traditional `sudo` through PTY. This provides better security, Touch ID support, and a more integrated user experience.

## Design Goals

1. **Security**: Password/biometrics never flow through the PTY
2. **Native UX**: Use system authentication dialogs (Touch ID, system password prompts)
3. **Visual Clarity**: Clear indication when commands run with elevated privileges
4. **Session Management**: Support for elevated sessions spanning multiple commands

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Nexus UI                              │
├─────────────────────────────────────────────────────────────┤
│  $ elevate apt update                                        │
│  ┌─────────────────────────────────────────┐                │
│  │  Nexus wants to run:                    │                │
│  │  "apt update"                           │                │
│  │                                         │                │
│  │  [Touch ID]  or  Password: [________]   │                │
│  │                                         │                │
│  │  [Cancel]                  [Authorize]  │                │
│  └─────────────────────────────────────────┘                │
│                                                              │
│  🔓 apt update                              ← elevated block │
│  Hit:1 http://archive.ubuntu.com/ubuntu jammy InRelease     │
│  Reading package lists... Done                               │
│                                                              │
│  $ _                                                         │
└─────────────────────────────────────────────────────────────┘
```

## Platform Implementation

### macOS

Use Security.framework's Authorization Services:

```rust
// Pseudocode
use security_framework::authorization::{Authorization, Flags, Rights};

fn elevate_command(command: &str) -> Result<()> {
    let auth = Authorization::new(
        Rights::from_str("system.privilege.admin")?,
        None,
        Flags::INTERACTION_ALLOWED | Flags::EXTEND_RIGHTS,
    )?;

    // Execute with elevated privileges
    auth.execute_with_privileges(command, &args)?;
}
```

Key APIs:
- `AuthorizationCreate` - Create authorization reference
- `AuthorizationCopyRights` - Request specific rights (triggers UI)
- `AuthorizationExecuteWithPrivileges` (deprecated but functional)
- Modern alternative: Use `SMJobBless` or XPC helper tool

**Touch ID Integration**: Authorization Services automatically offers Touch ID when available and appropriate for the requested right.

### Linux

Use polkit (PolicyKit) via D-Bus:

```rust
// Pseudocode - using zbus for D-Bus
async fn elevate_command(command: &str) -> Result<()> {
    let connection = Connection::system().await?;
    let proxy = PolkitAuthorityProxy::new(&connection).await?;

    let result = proxy.check_authorization(
        Subject::unix_process(std::process::id()),
        "org.freedesktop.policykit.exec",
        HashMap::new(),
        CheckAuthorizationFlags::AllowUserInteraction,
    ).await?;

    if result.is_authorized {
        // Execute via pkexec or privileged helper
    }
}
```

Alternative: Direct `pkexec` invocation with custom .policy file.

## User Interface

### Elevated Block Indicator

Blocks running with elevated privileges display a visual indicator:

```
┌─ 🔓 ────────────────────────────────────────────────────────┐
│ $ apt update                                                 │
│ Hit:1 http://archive.ubuntu.com/ubuntu jammy InRelease      │
│ ...                                                          │
└──────────────────────────────────────────────────────────────┘
```

Options:
- Lock/unlock icon (🔓)
- Colored border (red/orange)
- Background tint
- "ELEVATED" badge

### Elevated Session Mode

For multiple elevated commands, support a session mode:

```
$ elevate --session
🔓 Elevated session started (expires in 5 minutes)

🔓 $ apt update
...

🔓 $ apt upgrade -y
...

🔓 $ exit
Session ended.

$ _
```

## Commands

### `elevate` Builtin

```
elevate [OPTIONS] [COMMAND]

Options:
  --session, -s     Start an elevated session (multiple commands)
  --timeout MINS    Session timeout (default: 5 minutes)
  --reason TEXT     Reason shown in auth dialog

Examples:
  elevate rm -rf /var/cache/*     # Single elevated command
  elevate -s                       # Start elevated session
  elevate --reason "Install deps" make install
```

### Intercepting `sudo`

When user types `sudo command`:
1. Intercept before PTY execution
2. Strip `sudo` prefix
3. Route through native elevation
4. Execute command with privileges

This provides backward compatibility while using native auth.

## Security Considerations

### Advantages over PTY sudo

1. **No password in PTY stream**: Credentials handled entirely by OS
2. **Biometric support**: Touch ID, Windows Hello, etc.
3. **Audit trail**: System logs authentication attempts
4. **Credential isolation**: Shell process never sees credentials
5. **Rate limiting**: OS handles brute-force protection

### Credential Caching

- Defer to OS credential caching (Authorization Services caches by default)
- Elevated sessions use held authorization reference
- Clear on session end or timeout
- Never store credentials in shell state

### Sandboxing Considerations

If Nexus runs sandboxed (App Store):
- May need XPC helper tool for elevation
- Helper installed outside sandbox with elevated capabilities
- Communication via XPC for privileged operations

## Implementation Phases

### Phase 1: Basic Elevation
- [ ] `elevate` builtin command
- [ ] macOS Authorization Services integration
- [ ] Visual indicator for elevated blocks
- [ ] Basic error handling

### Phase 2: Session Support
- [ ] Elevated session mode
- [ ] Session timeout handling
- [ ] Session persistence across blocks

### Phase 3: Linux Support
- [ ] polkit integration
- [ ] Custom .policy file for Nexus
- [ ] pkexec wrapper

### Phase 4: Polish
- [ ] `sudo` interception (optional)
- [ ] Configurable timeout defaults
- [ ] Audit logging
- [ ] Touch ID preference handling

## Dependencies

### macOS
- `security-framework` crate (Rust bindings to Security.framework)
- Possibly `objc` for Authorization UI customization

### Linux
- `zbus` for D-Bus communication
- polkit development libraries
- Custom .policy file installation

## Open Questions

1. Should `sudo` be intercepted automatically, or require explicit `elevate`?
2. How to handle elevation for piped commands (`elevate cat /etc/shadow | grep root`)?
3. Should elevated sessions be visually distinct (different background color)?
4. How to handle elevation in non-interactive/scripted mode?
