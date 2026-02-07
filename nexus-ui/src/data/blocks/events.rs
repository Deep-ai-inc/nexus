//! PTY event types for communication with the PTY subprocess.

/// PTY event types for communication with the PTY subprocess.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(i32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pty_event_output() {
        let event = PtyEvent::Output(vec![65, 66, 67]);
        if let PtyEvent::Output(data) = event {
            assert_eq!(data, vec![65, 66, 67]);
        } else {
            panic!("Expected PtyEvent::Output");
        }
    }

    #[test]
    fn test_pty_event_exited() {
        let event = PtyEvent::Exited(0);
        if let PtyEvent::Exited(code) = event {
            assert_eq!(code, 0);
        } else {
            panic!("Expected PtyEvent::Exited");
        }
    }

    #[test]
    fn test_pty_event_clone() {
        let event = PtyEvent::Exited(1);
        let cloned = event.clone();
        if let PtyEvent::Exited(code) = cloned {
            assert_eq!(code, 1);
        }
    }
}
