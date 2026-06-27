//! D-Bus service for `io.rustyboard.Keyboard`.

use anyhow::Result;
use tracing::info;
use zbus::{interface, object_server::SignalContext};

pub struct KeyboardService {
    visible: bool,
}

#[interface(name = "io.rustyboard.Keyboard1")]
impl KeyboardService {
    /// Show the keyboard.
    async fn show(&mut self) -> bool {
        self.visible = true;
        info!("D-Bus: Show()");
        true
    }

    /// Hide the keyboard.
    async fn hide(&mut self) -> bool {
        self.visible = false;
        info!("D-Bus: Hide()");
        true
    }

    /// True iff the keyboard is currently visible.
    #[zbus(property)]
    fn visible(&self) -> bool {
        self.visible
    }

    /// Trigger voice-input recording.
    async fn start_voice(&self) -> bool {
        info!("D-Bus: StartVoice()");
        true
    }

    /// Emitted whenever the keyboard commits text. The `ctxt` argument is
    /// supplied by the `interface` macro at call sites; consumers invoke
    /// `Self::text_committed(&ctx, text).await` from inside their handlers.
    #[zbus(signal)]
    async fn text_committed(ctxt: &SignalContext<'_>, text: String) -> zbus::Result<()>;
}

/// Run the D-Bus service forever (called from a tokio runtime in main).
pub async fn run_service() -> Result<()> {
    let service = KeyboardService { visible: false };

    let _conn = zbus::connection::Builder::session()?
        .name("io.rustyboard.Keyboard")?
        .serve_at("/io/rustyboard/Keyboard", service)?
        .build()
        .await?;

    info!("D-Bus service ready at io.rustyboard.Keyboard");

    // Keep the connection alive forever.
    std::future::pending::<()>().await;
    Ok(())
}
