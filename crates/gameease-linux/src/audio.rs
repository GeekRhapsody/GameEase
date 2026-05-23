use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as AnyhowContext, Result};
use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::introspect::SinkInfo;
use pulse::context::{Context, FlagSet, State};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::proplist::{properties, Proplist};
use pulse::volume::{ChannelVolumes, Volume};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(3);
const MAINLOOP_SLEEP: Duration = Duration::from_millis(5);
const MAX_VOLUME_PERCENT: u8 = 100;

/// Thread-shareable PulseAudio controller handle.
pub type SharedAudioController = Arc<Mutex<AudioController>>;

/// Current system output volume state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AudioSnapshot {
    /// Output volume percentage.
    pub volume: u8,
    /// Whether the default output sink is muted.
    pub muted: bool,
}

/// PulseAudio-backed volume controller.
pub struct AudioController {
    sender: Sender<AudioCommand>,
}

impl AudioController {
    /// Starts the PulseAudio worker thread and returns a controller.
    pub fn new() -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("gameease-audio".to_string())
            .spawn(move || run_audio_thread(receiver))
            .context("failed to spawn audio thread")?;

        Ok(Self { sender })
    }

    /// Starts the PulseAudio worker thread behind `Arc<Mutex<_>>`.
    pub fn new_shared() -> Result<SharedAudioController> {
        Ok(Arc::new(Mutex::new(Self::new()?)))
    }

    /// Returns the default output volume clamped to 0-100%.
    #[allow(dead_code)]
    pub fn get_volume(&self) -> Result<u8> {
        Ok(self.get_snapshot()?.volume.min(100))
    }

    /// Returns the default output volume and mute state.
    pub fn get_snapshot(&self) -> Result<AudioSnapshot> {
        self.request(AudioCommand::GetSnapshot)
    }

    /// Sets the default output volume, clamped to 0-100%.
    pub fn set_volume(&self, pct: u8) -> Result<()> {
        self.request(|response| AudioCommand::SetVolume(pct.min(MAX_VOLUME_PERCENT), response))
    }

    /// Toggles mute on the default output sink.
    pub fn toggle_mute(&self) -> Result<()> {
        self.request(AudioCommand::ToggleMute)
    }

    fn request<T>(&self, build: impl FnOnce(Sender<Result<T, String>>) -> AudioCommand) -> Result<T>
    where
        T: Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(build(sender))
            .context("failed to send audio command")?;

        receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .context("timed out waiting for audio command")?
            .map_err(|error| anyhow!(error))
    }
}

enum AudioCommand {
    GetSnapshot(Sender<Result<AudioSnapshot, String>>),
    SetVolume(u8, Sender<Result<(), String>>),
    ToggleMute(Sender<Result<(), String>>),
}

struct PulseWorker {
    mainloop: Mainloop,
    context: Context,
}

struct SinkSnapshot {
    name: String,
    channels: u8,
    volume: u8,
    muted: bool,
}

fn run_audio_thread(receiver: Receiver<AudioCommand>) {
    match PulseWorker::connect() {
        Ok(mut worker) => worker.run(receiver),
        Err(error) => fail_audio_commands(receiver, error.to_string()),
    }
}

fn fail_audio_commands(receiver: Receiver<AudioCommand>, error: String) {
    for command in receiver {
        match command {
            AudioCommand::GetSnapshot(response) => {
                let _ = response.send(Err(error.clone()));
            }
            AudioCommand::SetVolume(_, response) | AudioCommand::ToggleMute(response) => {
                let _ = response.send(Err(error.clone()));
            }
        }
    }
}

impl PulseWorker {
    fn connect() -> Result<Self> {
        let mut mainloop = Mainloop::new().context("failed to create PulseAudio mainloop")?;
        let mut proplist = Proplist::new().context("failed to create PulseAudio proplist")?;
        proplist
            .set_str(properties::APPLICATION_NAME, "GameEase")
            .map_err(|_| anyhow!("failed to set PulseAudio application name"))?;

        let mut context = Context::new_with_proplist(&mainloop, "GameEase", &proplist)
            .context("failed to create PulseAudio context")?;
        context
            .connect(None, FlagSet::NOFLAGS, None)
            .map_err(|error| anyhow!("failed to connect to PulseAudio: {error:?}"))?;

        let started = Instant::now();
        loop {
            pump_mainloop(&mut mainloop)?;
            match context.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => {
                    return Err(anyhow!(
                        "PulseAudio context failed with state {:?}",
                        context.get_state()
                    ));
                }
                _ if started.elapsed() > OPERATION_TIMEOUT => {
                    return Err(anyhow!("timed out connecting to PulseAudio"));
                }
                _ => thread::sleep(MAINLOOP_SLEEP),
            }
        }

        Ok(Self { mainloop, context })
    }

    fn run(&mut self, receiver: Receiver<AudioCommand>) {
        while let Ok(command) = receiver.recv() {
            match command {
                AudioCommand::GetSnapshot(response) => {
                    let _ = response.send(self.get_snapshot().map_err(|error| error.to_string()));
                }
                AudioCommand::SetVolume(volume, response) => {
                    let _ =
                        response.send(self.set_volume(volume).map_err(|error| error.to_string()));
                }
                AudioCommand::ToggleMute(response) => {
                    let _ = response.send(self.toggle_mute().map_err(|error| error.to_string()));
                }
            }
        }

        self.context.disconnect();
    }

    fn get_snapshot(&mut self) -> Result<AudioSnapshot> {
        let sink = self.default_sink_snapshot()?;
        Ok(AudioSnapshot {
            volume: sink.volume,
            muted: sink.muted,
        })
    }

    fn set_volume(&mut self, pct: u8) -> Result<()> {
        let sink = self.default_sink_snapshot()?;
        let volume = volume_from_percent(pct.min(MAX_VOLUME_PERCENT));
        let mut channel_volumes = ChannelVolumes::default();
        channel_volumes.set(sink.channels.max(1), volume);

        let (sender, receiver) = mpsc::channel();
        let mut introspector = self.context.introspect();
        let _operation = introspector.set_sink_volume_by_name(
            &sink.name,
            &channel_volumes,
            Some(Box::new(move |success| {
                let _ = sender.send(success);
            })),
        );

        ensure_success(
            self.wait_for_value(receiver, "set sink volume")?,
            "set sink volume",
        )
    }

    fn toggle_mute(&mut self) -> Result<()> {
        let sink = self.default_sink_snapshot()?;
        let (sender, receiver) = mpsc::channel();
        let mut introspector = self.context.introspect();
        let _operation = introspector.set_sink_mute_by_name(
            &sink.name,
            !sink.muted,
            Some(Box::new(move |success| {
                let _ = sender.send(success);
            })),
        );

        ensure_success(
            self.wait_for_value(receiver, "toggle sink mute")?,
            "toggle sink mute",
        )
    }

    fn default_sink_snapshot(&mut self) -> Result<SinkSnapshot> {
        let sink_name = self.default_sink_name()?;
        self.sink_snapshot(&sink_name)
    }

    fn default_sink_name(&mut self) -> Result<String> {
        let (sender, receiver) = mpsc::channel();
        let introspector = self.context.introspect();
        let _operation = introspector.get_server_info(move |info| {
            let result = info
                .default_sink_name
                .as_ref()
                .map(|name| name.to_string())
                .ok_or_else(|| "PulseAudio did not report a default sink".to_string());
            let _ = sender.send(result);
        });

        self.wait_for_value(receiver, "get default sink")?
            .map_err(|error| anyhow!(error))
    }

    fn sink_snapshot(&mut self, sink_name: &str) -> Result<SinkSnapshot> {
        let (sender, receiver) = mpsc::channel();
        let introspector = self.context.introspect();
        let _operation =
            introspector.get_sink_info_by_name(sink_name, move |result| match result {
                ListResult::Item(info) => {
                    let _ = sender.send(Ok(snapshot_from_sink_info(info)));
                }
                ListResult::Error => {
                    let _ = sender.send(Err("failed to read default sink".to_string()));
                }
                ListResult::End => {}
            });

        self.wait_for_value(receiver, "get sink info")?
            .map_err(|error| anyhow!(error))
    }

    fn wait_for_value<T>(&mut self, receiver: Receiver<T>, action: &str) -> Result<T> {
        let started = Instant::now();
        loop {
            match receiver.try_recv() {
                Ok(value) => return Ok(value),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(anyhow!(
                        "PulseAudio callback disconnected while trying to {action}"
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }

            pump_mainloop(&mut self.mainloop)?;

            if started.elapsed() > OPERATION_TIMEOUT {
                return Err(anyhow!("timed out trying to {action}"));
            }

            thread::sleep(MAINLOOP_SLEEP);
        }
    }
}

fn snapshot_from_sink_info(info: &SinkInfo<'_>) -> SinkSnapshot {
    SinkSnapshot {
        name: info
            .name
            .as_ref()
            .map(|name| name.to_string())
            .unwrap_or_default(),
        channels: info.volume.len(),
        volume: percent_from_volume(info.volume.avg()).min(MAX_VOLUME_PERCENT),
        muted: info.mute,
    }
}

fn pump_mainloop(mainloop: &mut Mainloop) -> Result<()> {
    match mainloop.iterate(false) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(retval) => Err(anyhow!("PulseAudio mainloop quit with {retval:?}")),
        IterateResult::Err(error) => Err(anyhow!("PulseAudio mainloop failed: {error:?}")),
    }
}

fn percent_from_volume(volume: Volume) -> u8 {
    let percent = ((volume.0 as f64 / Volume::NORMAL.0 as f64) * 100.0).round();
    percent.clamp(0.0, MAX_VOLUME_PERCENT as f64) as u8
}

fn volume_from_percent(percent: u8) -> Volume {
    Volume(((Volume::NORMAL.0 as u64 * percent as u64) / 100) as u32)
}

fn ensure_success(success: bool, action: &str) -> Result<()> {
    if success {
        Ok(())
    } else {
        Err(anyhow!("PulseAudio failed to {action}"))
    }
}
