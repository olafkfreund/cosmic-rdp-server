// PipeWire stream handler for receiving video frames.
//
// Phase 2 will implement:
// - Dedicated PipeWire thread with its own MainLoop
// - SHM and DMA-BUF buffer handling
// - Frame delivery via tokio mpsc channel
// - Damage rectangle extraction from SPA metadata
