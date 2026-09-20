use bollard::container::LogOutput;
use dockui::exec::{attached_session, encode_key};
use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWrite};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

type LogStream =
    Pin<Box<dyn futures_util::Stream<Item = Result<LogOutput, bollard::errors::Error>> + Send>>;

struct ChanWriter(UnboundedSender<Vec<u8>>);

impl AsyncWrite for ChanWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let _ = self.0.send(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn tty_console_frames_reach_the_terminal() {
    let frames: LogStream = Box::pin(futures_util::stream::iter(vec![
        Ok(LogOutput::Console {
            message: bytes::Bytes::from_static(b"sh-5.2$ "),
        }),
        Ok(LogOutput::StdOut {
            message: bytes::Bytes::from_static(b"out\n"),
        }),
        Ok(LogOutput::StdErr {
            message: bytes::Bytes::from_static(b"err\n"),
        }),
    ]));
    let (out_w, mut out_r) = tokio::io::duplex(4096);
    let (in_tx, mut in_rx) = unbounded_channel::<CEvent>();
    let (keys_tx, mut keys_rx) = unbounded_channel::<Vec<u8>>();

    attached_session(
        frames,
        Box::pin(out_w),
        Box::pin(ChanWriter(keys_tx)),
        &mut in_rx,
    )
    .await
    .unwrap();
    drop(in_tx);

    let mut seen = Vec::new();
    out_r.read_to_end(&mut seen).await.unwrap();
    assert_eq!(seen, b"sh-5.2$ out\nerr\n");
    assert!(keys_rx.try_recv().is_err());
}

#[tokio::test]
async fn key_events_are_forwarded_and_close_ends_the_session() {
    let frames: LogStream = Box::pin(futures_util::stream::pending());
    let (out_w, _out_r) = tokio::io::duplex(64);
    let (in_tx, mut in_rx) = unbounded_channel::<CEvent>();
    let (keys_tx, mut keys_rx) = unbounded_channel::<Vec<u8>>();

    in_tx
        .send(CEvent::Key(KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::NONE,
        )))
        .unwrap();
    in_tx
        .send(CEvent::Key(KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::NONE,
        )))
        .unwrap();
    in_tx
        .send(CEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
        .unwrap();
    drop(in_tx);

    attached_session(
        frames,
        Box::pin(out_w),
        Box::pin(ChanWriter(keys_tx)),
        &mut in_rx,
    )
    .await
    .unwrap();

    let mut sent = Vec::new();
    while let Ok(v) = keys_rx.try_recv() {
        sent.extend_from_slice(&v);
    }
    assert_eq!(sent, b"ls\r");
}

#[test]
fn encode_key_maps_control_alt_navigation_and_release() {
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(encode_key(ctrl_c), vec![0x03]);

    let alt_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
    assert_eq!(encode_key(alt_x), vec![0x1b, b'x']);

    assert_eq!(
        encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        b"\x1b[A".to_vec()
    );
    assert_eq!(
        encode_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        b"\r".to_vec()
    );

    let release = KeyEvent {
        kind: KeyEventKind::Release,
        ..KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
    };
    assert!(encode_key(release).is_empty());
}
