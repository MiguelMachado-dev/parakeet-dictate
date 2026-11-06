use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use log::error;

use crate::meter::LevelMeter;

#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub rate: u32,
}

impl Audio {
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(self.rate)
    }
}

pub trait Record: Send {
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<Audio, String>;
    fn discard(&mut self);
    fn device_name(&self) -> String;
}

/// The meter of the current recording, replaced when the stream's rate changes, so the overlay always
/// reads levels at the rate the stream actually runs.
#[derive(Clone)]
pub struct CurrentMeter(Arc<Mutex<Arc<LevelMeter>>>);

impl CurrentMeter {
    pub fn get(&self) -> Arc<LevelMeter> {
        self.0.lock().unwrap().clone()
    }

    fn for_rate(&self, rate: u32) -> Arc<LevelMeter> {
        let mut meter = self.0.lock().unwrap();
        if meter.rate == rate {
            meter.reset();
        } else {
            *meter = Arc::new(LevelMeter::new(rate));
        }
        meter.clone()
    }
}

pub fn default_input_rate() -> u32 {
    let device = cpal::default_host().default_input_device();
    device.and_then(|device| device.default_input_config().ok()).map_or(16_000, |config| config.sample_rate())
}

pub struct Microphone {
    stream: Option<Stream>,
    chunks: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    device_name: String,
    meter: CurrentMeter,
}

impl Default for Microphone {
    fn default() -> Self {
        Self::new()
    }
}

impl Microphone {
    pub fn new() -> Self {
        Self {
            stream: None,
            chunks: Arc::new(Mutex::new(Vec::new())),
            rate: 16_000,
            device_name: String::new(),
            meter: CurrentMeter(Arc::new(Mutex::new(Arc::new(LevelMeter::new(16_000))))),
        }
    }

    pub fn meter(&self) -> CurrentMeter {
        self.meter.clone()
    }

    fn build<T>(&self, device: &cpal::Device, config: StreamConfig, meter: Arc<LevelMeter>) -> Result<Stream, String>
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        let chunks = self.chunks.clone();
        let channels = usize::from(config.channels).max(1);
        let mut mono: Vec<f32> = Vec::new();
        let on_audio = move |data: &[T], _: &cpal::InputCallbackInfo| {
            mono.clear();
            mono.extend(data.chunks_exact(channels).map(|frame| {
                frame.iter().map(|sample| f32::from_sample_(*sample)).sum::<f32>() / channels as f32
            }));
            chunks.lock().unwrap().extend_from_slice(&mono);
            meter.feed(&mono);
        };
        let on_error = |e: cpal::Error| error!("audio stream: {e}");
        device.build_input_stream(config, on_audio, on_error, None).map_err(|e| e.to_string())
    }
}

impl Record for Microphone {
    fn start(&mut self) -> Result<(), String> {
        self.discard();
        // Asked anew on each start, so a headset connected after launch is picked up as the default.
        let device = cpal::default_host().default_input_device().ok_or("no microphone")?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let name = device.description().map_or_else(|_| "microphone".to_owned(), |description| description.name().to_owned());
        let rate = supported.sample_rate();
        let meter = self.meter.for_rate(rate);
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => self.build::<f32>(&device, config, meter),
            SampleFormat::I16 => self.build::<i16>(&device, config, meter),
            SampleFormat::I32 => self.build::<i32>(&device, config, meter),
            SampleFormat::U8 => self.build::<u8>(&device, config, meter),
            other => Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        self.rate = rate;
        self.device_name = name;
        Ok(())
    }

    fn stop(&mut self) -> Result<Audio, String> {
        let Some(stream) = self.stream.take() else {
            return Err("stop() without start()".to_owned());
        };
        drop(stream);
        let samples = std::mem::take(&mut *self.chunks.lock().unwrap());
        if samples.is_empty() {
            return Err(format!("no audio from '{}'", self.device_name));
        }
        Ok(Audio { samples, rate: self.rate })
    }

    fn discard(&mut self) {
        self.stream = None;
        self.chunks.lock().unwrap().clear();
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }
}
