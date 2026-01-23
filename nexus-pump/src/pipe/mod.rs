//! Pump threads and ring buffers for pipeline observation.

mod ring_buffer;

pub use ring_buffer::RingBuffer;

use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::Sender;
use nexus_api::{BlockId, ShellEvent};

use crate::sniffer;

/// A pump that copies data from a reader to a writer while observing it.
pub struct Pump {
    /// Handle to the pump thread.
    handle: Option<JoinHandle<io::Result<()>>>,

    /// Signal to stop the pump.
    stop: Arc<AtomicBool>,

    /// The ring buffer containing observed data.
    buffer: Arc<RingBuffer>,
}

impl Pump {
    /// Spawn a new pump thread.
    pub fn spawn(
        reader: File,
        writer: Option<File>,
        block_id: BlockId,
        events: Sender<ShellEvent>,
        buffer_size: usize,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let buffer = Arc::new(RingBuffer::new(buffer_size));

        let stop_clone = stop.clone();
        let buffer_clone = buffer.clone();

        let handle = thread::spawn(move || {
            pump_loop(reader, writer, block_id, events, buffer_clone, stop_clone)
        });

        Self {
            handle: Some(handle),
            stop,
            buffer,
        }
    }

    /// Get a reference to the ring buffer.
    pub fn buffer(&self) -> &Arc<RingBuffer> {
        &self.buffer
    }

    /// Stop the pump and wait for it to finish.
    pub fn stop(mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| {
                io::Error::new(io::ErrorKind::Other, "pump thread panicked")
            })??;
        }
        Ok(())
    }
}

/// The main pump loop.
fn pump_loop(
    mut reader: File,
    mut writer: Option<File>,
    block_id: BlockId,
    events: Sender<ShellEvent>,
    buffer: Arc<RingBuffer>,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    let mut temp_buf = [0u8; 8192];
    let mut total_bytes = 0u64;
    let mut format_detected = false;

    while !stop.load(Ordering::SeqCst) {
        match reader.read(&mut temp_buf) {
            Ok(0) => break, // EOF
            Ok(n) => {
                let data = &temp_buf[..n];

                // Critical path: write to destination first
                if let Some(ref mut w) = writer {
                    w.write_all(data)?;
                }

                // Then copy to ring buffer (non-blocking)
                buffer.write(data);
                total_bytes += n as u64;

                // Emit event for UI
                let _ = events.send(ShellEvent::StdoutChunk {
                    block_id,
                    data: data.to_vec(),
                });

                // Sniff format on first chunk
                if !format_detected && total_bytes >= 512 {
                    let sample = buffer.read_all();
                    let format = sniffer::detect_format(&sample);
                    tracing::debug!("detected format: {:?}", format);
                    format_detected = true;
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }

    Ok(())
}
