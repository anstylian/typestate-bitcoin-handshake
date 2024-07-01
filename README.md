# Bitcoin handshake implemented using typestate pattern

## Why Typestate?
Typestate is a design pattern that allows as to leverage the type system 
and describe our flow. The benefit is that at compile time we will know 
that our flow is valid

## Bitcoin handshake
The handshake of bitcoin is simple. 

```text
 ┌─────┐               ┌──────┐
 │Local│               │Remote│
 └──┬──┘               └───┬──┘
    │       Version        │
    │─────────────────────►│
    │       VerAck         │
    │◄─────────────────────│
    │       Version        │
    │◄─────────────────────│
    │       VerAck         │
    │─────────────────────►│
    │                      │
 ┌──┴──┐               ┌───┴──┐
 │Local│               │Remote│
 └─────┘               └──────┘
```

The protocol has no guarantees for the messages order. 
The `Version` message of the local host is the first message that starts the handshake, but we can not 
know if the `VerAck` or the `Version` will come first. We just need to make sure that each `Version` message
will be followed by an `VerAck` message.

## Typestate
```tet
Init 
  -> Sent Version
  -> choice Receive VerAck Or Version {	
    VerAck
      -> Wait for Version
      -> Sent VerAck
    Version
      -> Sent VerAck
      -> Wait for VerAck
  }
  -> Complete
```

## State Machine
The state machine that is develop in the code.

```text
                                ┌────────┐      ┌──────┐
┌───────┐       ┌───────┐       │Choice  ├─────►│VerAck│
│Initial├──────►│Version├──────►│ VerAck │      └────┬─┘
└───────┘       └───────┘       │   or   │           │
                                │ Version│           ▼
                                └────┬───┘         ┌────────┐
                                     ▼             │Wait for│
                                 ┌───────┐         │Version │
                                 │Version│         └─┬──────┘
                                 └───┬───┘           ▼
                                     ▼             ┌───────────┐
                                 ┌───────────┐     │Send VerAck│
                                 │Send VerAck│     └───┬───────┘
                                 └───┬───────┘         │
                                     ▼                 ▼
                                 ┌───────────┐       ┌────────┐
                                 │Wait VerArk├──────►│Complete│
                                 └───────────┘       └────────┘
```

## How to run
Assuming you already have Rust installed.

### Using `cargo`

```
cargo run -- <node_ip_address:port> <node_ip_address:port> <node_ip_address:port>
```

## Documentation
You can generate and view the project documentation using `cargo doc --open`.
