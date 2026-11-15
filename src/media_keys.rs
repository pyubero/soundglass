//! The keyboard's media keys while soundglass isn't focused: the desktop sends them to the
//! active media player, which on Linux is an MPRIS player on D-Bus and on Windows a System Media
//! Transport Controls session. Focused-window keys are read by `player_keys` in main.rs.

use std::sync::mpsc::{Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    #[cfg_attr(windows, allow(dead_code))] // SMTC sends Play or Pause by the status we report.
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

pub struct MediaKeys {
    pub keys: Receiver<Key>,
    /// Tells the desktop whether we're playing (it picks the player to send keys to by it).
    status: Sender<bool>,
    sent: Option<bool>,
}

impl MediaKeys {
    pub fn set_playing(&mut self, playing: bool) {
        if self.sent != Some(playing) {
            self.sent = Some(playing);
            let _ = self.status.send(playing);
        }
    }
}

/// Registers with the desktop on a thread of its own, so the window doesn't wait for it. When
/// there's nothing to register with (no session bus, an old Windows, another OS) the thread ends
/// and no keys ever come.
pub fn start(ctx: eframe::egui::Context) -> MediaKeys {
    let (key_tx, keys) = std::sync::mpsc::channel();
    let (status, status_rx) = std::sync::mpsc::channel();
    let send = move |k| {
        let _ = key_tx.send(k);
        ctx.request_repaint();
    };
    let _ = std::thread::Builder::new().name("media keys".into()).spawn(move || serve(send, status_rx));
    MediaKeys { keys, status, sent: None }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn serve(_send: impl Fn(Key), _status: Receiver<bool>) {}

#[cfg(target_os = "linux")]
fn serve(send: impl Fn(Key) + Send + Sync + 'static, status: Receiver<bool>) {
    use zbus::interface;

    struct Root;

    #[interface(name = "org.mpris.MediaPlayer2")]
    impl Root {
        fn raise(&self) {}
        fn quit(&self) {}
        #[zbus(property)]
        fn can_quit(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn can_raise(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn has_track_list(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn identity(&self) -> &str {
            "soundglass"
        }
        #[zbus(property)]
        fn desktop_entry(&self) -> &str {
            "soundglass"
        }
        #[zbus(property)]
        fn supported_uri_schemes(&self) -> Vec<String> {
            Vec::new()
        }
        #[zbus(property)]
        fn supported_mime_types(&self) -> Vec<String> {
            Vec::new()
        }
    }

    struct Player {
        send: Box<dyn Fn(Key) + Send + Sync>,
        playing: bool,
    }

    #[interface(name = "org.mpris.MediaPlayer2.Player")]
    impl Player {
        fn play_pause(&self) {
            (self.send)(Key::PlayPause);
        }
        fn play(&self) {
            (self.send)(Key::Play);
        }
        fn pause(&self) {
            (self.send)(Key::Pause);
        }
        fn stop(&self) {
            (self.send)(Key::Stop);
        }
        fn next(&self) {
            (self.send)(Key::Next);
        }
        fn previous(&self) {
            (self.send)(Key::Previous);
        }
        #[zbus(property)]
        fn playback_status(&self) -> &str {
            if self.playing { "Playing" } else { "Paused" }
        }
        #[zbus(property)]
        fn rate(&self) -> f64 {
            1.0
        }
        #[zbus(property)]
        fn metadata(&self) -> std::collections::HashMap<String, zbus::zvariant::OwnedValue> {
            Default::default()
        }
        #[zbus(property)]
        fn can_control(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_play(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_pause(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_go_next(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_go_previous(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_seek(&self) -> bool {
            false
        }
    }

    const PATH: &str = "/org/mpris/MediaPlayer2";
    zbus::block_on(async move {
        let built = async {
            let conn = zbus::connection::Builder::session()?
                .serve_at(PATH, Root)?
                .serve_at(PATH, Player { send: Box::new(send), playing: false })?
                .build()
                .await?;
            // A second soundglass takes the spec's per-instance name.
            // Without DoNotQueue a second request waits in line and gets no name.
            let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
            if conn.request_name_with_flags("org.mpris.MediaPlayer2.soundglass", flags).await.ok() != Some(zbus::fdo::RequestNameReply::PrimaryOwner) {
                conn.request_name(format!("org.mpris.MediaPlayer2.soundglass.instance{}", std::process::id())).await?;
            }
            zbus::Result::Ok(conn)
        };
        let Ok(conn) = built.await else { return };
        let Ok(player) = conn.object_server().interface::<_, Player>(PATH).await else { return };
        // zbus answers calls on its own executor thread, so waiting here blocks nothing; the
        // channel closes when the app quits, which ends this thread and drops the connection.
        while let Ok(playing) = status.recv() {
            player.get_mut().await.playing = playing;
            let _ = player.get().await.playback_status_changed(player.signal_emitter()).await;
        }
    });
}

#[cfg(windows)]
fn serve(send: impl Fn(Key) + Send + Sync + 'static, status: Receiver<bool>) {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Playback::MediaPlayer;
    use windows::Media::{MediaPlaybackStatus, SystemMediaTransportControls, SystemMediaTransportControlsButton as B, SystemMediaTransportControlsButtonPressedEventArgs};

    // A desktop app has no SMTC of its own; an idle MediaPlayer lends one, with its automatic
    // commands off so only our handler answers the keys.
    let setup = || -> windows::core::Result<(MediaPlayer, SystemMediaTransportControls)> {
        unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED)? };
        let player = MediaPlayer::new()?;
        player.CommandManager()?.SetIsEnabled(false)?;
        let smtc = player.SystemMediaTransportControls()?;
        smtc.SetIsEnabled(true)?;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsStopEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        smtc.ButtonPressed(&TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let key = match args.Button()? {
                    B::Play => Key::Play,
                    B::Pause => Key::Pause,
                    B::Stop => Key::Stop,
                    B::Next => Key::Next,
                    B::Previous => Key::Previous,
                    _ => return Ok(()),
                };
                send(key);
            }
            Ok(())
        }))?;
        Ok((player, smtc))
    };
    // The SMTC lives as long as the MediaPlayer that lent it, so both stay here.
    let Ok((_player, smtc)) = setup() else { return };
    while let Ok(playing) = status.recv() {
        let _ = smtc.SetPlaybackStatus(if playing { MediaPlaybackStatus::Playing } else { MediaPlaybackStatus::Paused });
    }
}
