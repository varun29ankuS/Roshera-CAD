# ClientMessage/ServerMessage Protocol Architecture

## ⚠️ IMPORTANT: Protocol vs Transport Distinction

This directory implements the **ClientMessage/ServerMessage protocol**, which is the APPLICATION-LEVEL protocol for client-server communication.

### Key Concepts:
- **Protocol**: ClientMessage/ServerMessage (the message format and structure)
- **Transport**: WebSocket (just the delivery mechanism at `/ws` endpoint)

### DO NOT CONFUSE:
- ❌ "WebSocket protocol" - INCORRECT, WebSocket is just transport
- ✅ "ClientMessage protocol" - CORRECT, this is our actual protocol
- ✅ "ClientMessage sent over WebSocket" - CORRECT and precise

## Architecture Overview

```
┌─────────────┐                              ┌─────────────┐
│   Frontend  │                              │   Backend   │
│             │                              │             │
│  Sends:     │  ----[ WebSocket Transport ]--->  Receives: │
│ClientMessage│         at /ws endpoint       │ClientMessage│
│             │                              │             │
│  Receives:  │  <---[ WebSocket Transport ]----  Sends:    │
│ServerMessage│                              │ServerMessage│
└─────────────┘                              └─────────────┘
```

## Protocol Structure

### ClientMessage (Frontend → Backend)
```rust
pub enum ClientMessage {
    // Authentication
    Authenticate { token: String, request_id: Option<String> },
    
    // Feature-specific commands
    GeometryCommand { command: GeometryWSCommand, request_id: Option<String> },
    TimelineCommand { command: TimelineWSCommand, request_id: Option<String> },
    ExportCommand { command: ExportWSCommand, request_id: Option<String> },
    AICommand { command: AIWSCommand, request_id: Option<String> },
    SessionCommand { command: SessionWSCommand, request_id: Option<String> },
    // ... more variants
}
```

### ServerMessage (Backend → Frontend)
```rust
pub enum ServerMessage {
    Welcome { ... },
    Success { result: Option<Value>, request_id: Option<String> },
    Error { error_code: String, message: String, ... },
    // ... more variants
}
```

## Vision Commands Example

Vision commands are part of the ClientMessage protocol under AICommand:

```rust
ClientMessage::AICommand {
    command: AIWSCommand::ProcessCommandWithVision {
        text: "select the red box",
        viewport: ViewportCapture { ... },
        context: None
    },
    request_id: Some("req_123")
}
```

## File Organization

- `protocol.rs` - Defines ClientMessage/ServerMessage enums and all command types
- `handlers.rs` - Handles incoming ClientMessage and sends ServerMessage responses
- `mod.rs` - Module exports

## For AI Assistants and Developers

When discussing this system:
1. **Always refer to it as "ClientMessage protocol" not "WebSocket protocol"**
2. **WebSocket is just the transport layer, not the protocol**
3. **The protocol is ClientMessage (client→server) and ServerMessage (server→client)**
4. **Vision integration is through ClientMessage::AICommand, not a separate system**

## Common Misconceptions to Avoid

❌ **WRONG**: "The WebSocket handlers process vision commands"
✅ **RIGHT**: "The ClientMessage handlers process vision commands sent over WebSocket"

❌ **WRONG**: "WebSocket protocol includes ProcessCommandWithVision"
✅ **RIGHT**: "ClientMessage protocol includes ProcessCommandWithVision in AICommand"

❌ **WRONG**: "Vision uses a different WebSocket endpoint"
✅ **RIGHT**: "Vision commands use the same ClientMessage protocol at /ws endpoint"