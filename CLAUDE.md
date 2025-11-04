# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rustorrent is an async BitTorrent client implementation in Rust. The project uses Tokio for async runtime and implements the core BitTorrent protocol for downloading files from peers in a decentralized network.

## Build and Development Commands

### Building the Project
```bash
cargo build
```

### Running Tests
```bash
# Run all tests
cargo test

# Run a single test
cargo test test_name -- --test-threads=1

# Run tests with debug logging
RUST_LOG=debug cargo test

# Run a specific integration test
cargo test --test integration_test_name
```

### Running the Client
```bash
# Run the torrent client with a .torrent file
cargo run -- path/to/file.torrent

# Run with logging enabled
RUST_LOG=info cargo run -- path/to/file.torrent
RUST_LOG=debug cargo run -- path/to/file.torrent
```

### Code Quality
```bash
# Check code without building
cargo check

# Format code
cargo fmt

# Run clippy linter
cargo clippy

# Check the entire workspace
cargo check --all
```

## Architecture Overview

### High-Level Design

The client follows an **event-driven architecture** where all operations flow through a central event loop. The core workflow is:

1. **Torrent Parsing** → **Tracker Communication** → **Peer Discovery** → **Block Downloading** → **File Assembly**

### Key Components

#### 1. **Client** (`src/client/mod.rs`)
- Central orchestrator that spawns the main async task
- Manages the event loop using `tokio::select!`
- Coordinates between `EventHandler`, `CommandExecutor`, and `Timers`
- Sends notifications (download progress, completion) to the application

#### 2. **Event System** (`src/event/`)
- **EventHandler** (`event/handler.rs`): Processes events from peers, timers, and connections
  - Integrates three key decision systems: Choker, Scheduler, and Sweeper
  - Maintains global download statistics
  - Converts events into commands
- **Event Types**: Keep-alive ticks, choke ticks, stats updates, sweep ticks, message received, connection accepted, connection requested, shutdown
- **Sweeper** (`event/sweeper.rs`): Detects idle peers and stalled block requests

#### 3. **Scheduler** (`src/scheduler/`)
- **Purpose**: Manages piece selection and block assignment
- **Strategy**: Rarest-first piece selection (prioritizes pieces less available in swarm)
- **Components**:
  - **AvailablePieces**: Tracks which pieces are available from which peers
  - **ActivePieces**: Manages pieces currently being downloaded
  - **BlockManager**: Handles individual 16KB block assignments
  - **PieceState**: Tracks state of each piece (pending, in-progress, completed)
- **Key Methods**: `peer_unchoked()` assigns blocks, `receive_block()` updates progress, `release()` handles timeouts

#### 4. **Peer Management** (`src/peer/`)
- **ConnectionManager** (`peer/connection_manager.rs`): Manages all peer connections
  - Spawns and terminates peer connection tasks
  - Routes messages to/from peers
- **Connection** (`peer/connection.rs`): Individual peer connection handler
  - Handles handshake, message I/O with peer
  - Manages lifecycle (connect, ready, active, shutdown)
- **Choker** (`peer/choke.rs`): Implements BitTorrent choking/unchoking algorithm
  - Unchokes best 4 peers every 10 seconds
  - Optimistic unchoking of random peer
  - Tracks upload rates for peer selection
- **GlobalStats** (`peer/stats.rs`): Aggregates download/upload statistics

#### 5. **Message Protocol** (`src/message/`)
- **Codec**: Encodes/decodes BitTorrent protocol messages
- **Handshake**: Peer handshake with info_hash and peer_id validation
- **Message Types**: Keep-alive, Choke, Unchoke, Interested, NotInterested, Have, Bitfield, Request, Piece, Cancel, Reject

#### 6. **Bencoding** (`src/bencoding/`)
- Encodes/decodes bencoded data (used in .torrent files and tracker responses)
- **Value Types**: Integer, Byte string, List, Dictionary
- Used by torrent parser and tracker communication

#### 7. **Storage** (`src/storage/`)
- **Reader/Writer**: Async file I/O for reading and writing piece data
- **Joiner**: Validates downloaded pieces against SHA-1 hashes from torrent metadata
- Sparse file handling (pre-allocates download file)

#### 8. **Tracker** (`src/tracker/`)
- **Purpose**: Communicates with BitTorrent tracker to discover peers
- **Workflow**: Sends announce requests with download progress
- **Response Handling**: Parses peer list from tracker
- **Events**: Sends "started", periodic updates, and "stopped" (on shutdown)
- **Progress Tracking**: Uses watch channel to sync download state with tracker requests

#### 9. **Core Utilities** (`src/core/`)
- **Codec**: Low-level encoding/decoding for protocol messages
- **Crypto**: MD5 and SHA-1 hashing
- **PeerId**: Random peer identifier generation
- **TransferRate**: Calculates download/upload speeds
- **AsyncDecoder**: Async file reading for bencoding

#### 10. **Command Execution** (`src/command/`)
- **CommandExecutor**: Executes commands from EventHandler
- **Command Types**:
  - `StartConnection`: Connect to a peer
  - `Send`: Send message to peer
  - `Broadcast`: Send to all peers
  - `UpdateStats`: Update and publish statistics
  - `RequestBlock`: Request data block from peer
  - `Disconnect`: Close peer connection
  - `Stop`: Shutdown client

### Event Flow Diagram

```
Timer Events          Network Events
    ↓                      ↓
    └──→ Timers ←────────────┘
         ↓
    EventHandler (processes event)
         ↓
    Choker | Scheduler | Sweeper (make decisions)
         ↓
    Commands (what to do)
         ↓
    CommandExecutor (executes)
         ↓
    ConnectionManager (manages peer connections)
         ↓
    Peers (network I/O)
```

### Important Design Patterns

1. **Arc-wrapped shared state**: Download metadata and config are Arc'd for sharing across tasks
2. **Channel-based communication**: Events flow through mpsc channels between components
3. **Async/await with Tokio**: All I/O is non-blocking
4. **Task spawning**: Each peer connection runs as a separate spawned task
5. **Watch channels**: Tracker uses watch channel to sync download progress
6. **CancellationToken**: Graceful shutdown coordination

## Testing Strategy

- **Integration test** (`client::tests::one_seeder_one_leecher`): Tests seeder-leecher scenario with mock tracker
- **Mock tracker**: Uses `wiremock` to stub tracker HTTP responses
- **MD5 validation**: Tests verify downloaded files match source via MD5 checksum
- Test torrent with 6 pieces (32KB each) of "Alice in Wonderland" text

## Common Development Tasks

### Adding a New Protocol Message
1. Define message in `message/mod.rs`
2. Update `Codec` in `message/codec.rs` for encoding/decoding
3. Handle in `EventHandler::handle()` if it triggers state changes
4. Update peer connection if it affects peer state

### Debugging Peer Communications
- Set `RUST_LOG=trace` to see all message exchanges
- Check `peer/connection.rs` for handshake and message I/O
- Use `message/codec.rs` to trace protocol compliance

### Adding Tracker Features
- Modify `tracker/request.rs` for new announce parameters
- Update `tracker/response.rs` for parsing additional fields
- Watch channel in `Tracker::spawn()` manages state syncing

### Performance Profiling
- Monitor upload/download rates via `Stats` notifications
- Adjust config constants in `client/config.rs` (intervals, timeouts, buffer sizes)
- `transfer_rate.rs` calculates instantaneous rates for optimization analysis

## Configuration

Edit `client/config.rs` to adjust:
- `port`: Server listening port (default: 6881)
- `download_path`: Where to save downloaded files
- `channel_buffer`: Event channel buffer size
- `keep_alive_interval`: How often to send keep-alives
- `unchoking_interval`: How often to re-evaluate choking
- `idle_peer_timeout`: Disconnect peers after inactivity
- `block_timeout`: Consider block request stalled after timeout
- `optimistic_choking_cycle`: How often to optimistically unchoke a random peer
