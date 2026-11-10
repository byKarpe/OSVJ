use eframe::egui::{Color32, ColorImage};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const THUMB_W: usize = 160;
pub const THUMB_H: usize = 90;
pub const PREVIEW_W: usize = 320;
pub const PREVIEW_H: usize = 180;

pub fn silent_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Clone, Debug)]
pub struct WindowsMonitor {
    pub index: usize,
    pub device_name: String,
    pub is_primary: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowsMonitor {
    pub fn display_label(&self) -> String {
        let role = if self.is_primary {
            "Principal (Vista OSVJ)"
        } else {
            "Pantalla Externa"
        };
        format!(
            "Pantalla {} [{}] — {}x{} (Pos: {},{}) · {}",
            self.index + 1,
            self.device_name,
            self.width,
            self.height,
            self.x,
            self.y,
            role
        )
    }

    pub fn enumerate_windows_displays() -> Vec<WindowsMonitor> {
        let ps_cmd = "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { \"$($_.DeviceName)|$($_.Primary)|$($_.Bounds.X)|$($_.Bounds.Y)|$($_.Bounds.Width)|$($_.Bounds.Height)\" }";
        let mut list = Vec::new();

        if let Ok(output) = silent_command("powershell")
            .args(["-NoProfile", "-Command", ps_cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for (idx, line) in text.lines().enumerate() {
                let parts: Vec<&str> = line.trim().split('|').collect();
                if parts.len() == 6 {
                    let dev = parts[0].replace("\\\\.\\", "");
                    let primary = parts[1].eq_ignore_ascii_case("true");
                    let x = parts[2].parse::<i32>().unwrap_or(0);
                    let y = parts[3].parse::<i32>().unwrap_or(0);
                    let w = parts[4].parse::<u32>().unwrap_or(1920);
                    let h = parts[5].parse::<u32>().unwrap_or(1080);
                    list.push(WindowsMonitor {
                        index: idx,
                        device_name: dev,
                        is_primary: primary,
                        x,
                        y,
                        width: w,
                        height: h,
                    });
                }
            }
        }

        list.sort_by_key(|m| (!m.is_primary, m.x));
        for (i, m) in list.iter_mut().enumerate() {
            m.index = i;
        }

        if list.is_empty() {
            list.push(WindowsMonitor {
                index: 0,
                device_name: "DISPLAY1".into(),
                is_primary: true,
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            });
        }
        if list.len() == 1 {
            let w0 = list[0].width as i32;
            list.push(WindowsMonitor {
                index: 1,
                device_name: "DISPLAY2 (HDMI/LED)".into(),
                is_primary: false,
                x: w0,
                y: 0,
                width: 1920,
                height: 1080,
            });
            list.push(WindowsMonitor {
                index: 2,
                device_name: "DISPLAY3 (Aux)".into(),
                is_primary: false,
                x: w0 * 2,
                y: 0,
                width: 1920,
                height: 1080,
            });
        } else if list.len() == 2 {
            let last_end = list[1].x + list[1].width as i32;
            list.push(WindowsMonitor {
                index: 2,
                device_name: "DISPLAY3 (Aux)".into(),
                is_primary: false,
                x: last_end,
                y: 0,
                width: 1920,
                height: 1080,
            });
        }

        list
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum BlendMode {
    Alpha,
    Add,
    Multiply,
    Screen,
    Lighten,
}

impl BlendMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlendMode::Alpha => "Alpha",
            BlendMode::Add => "50 Add",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Lighten => "Lighten",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum BusAssign {
    Through,
    CrossA,
    CrossB,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum SliceShape {
    Rectangle,
    Circle,
    Triangle,
    Hexagon,
}

impl SliceShape {
    pub fn as_str(&self) -> &'static str {
        match self {
            SliceShape::Rectangle => "⬛ Rectángulo",
            SliceShape::Circle => "⚪ Círculo / Domo",
            SliceShape::Triangle => "🔺 Triángulo",
            SliceShape::Hexagon => "⬡ Hexágono",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum SliceSource {
    MasterComposition,
    Layer(usize),
}

impl SliceSource {
    pub fn label(&self, layer_names: &[String]) -> String {
        match self {
            SliceSource::MasterComposition => "Composición Master".to_string(),
            SliceSource::Layer(idx) => layer_names
                .get(*idx)
                .cloned()
                .unwrap_or_else(|| format!("Capa {}", idx + 1)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum GeneratorType {
    NeonTunnel,
    CyberGrid,
    SaberPulse,
    PlasmaWave,
    SliceStrobe,
    FireBorders,
}

impl GeneratorType {
    pub fn name(&self) -> &'static str {
        match self {
            GeneratorType::NeonTunnel => "AstroPulse_DXV3.mov",
            GeneratorType::CyberGrid => "CyberGrid_DXV3.mov",
            GeneratorType::SaberPulse => "SABER_FX_DXV3.mov",
            GeneratorType::PlasmaWave => "Intric8_Knot_DXV3.mov",
            GeneratorType::SliceStrobe => "SLICE_STROBE_DXV3.mov",
            GeneratorType::FireBorders => "Fire_Borders_DXV3.mov",
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum DecodeStatus {
    StandbyThumbReady,
    StreamingActive(usize, usize),
    MissingFfmpeg,
    DecodeError(String),
}

/// Streamer Bajo Demanda (On-Demand): en la grilla usa 0% CPU y 0 procesos; solo abre el stream cuando sale al aire
pub struct OnDemandVideoStream {
    pub path: PathBuf,
    pub latest_frame: Arc<Mutex<Option<ColorImage>>>,
    pub thumb_frame: Arc<Mutex<Option<ColorImage>>>,
    pub status: Arc<Mutex<DecodeStatus>>,
    pub thumb_rev: Arc<AtomicUsize>,
    pub is_streaming: Arc<AtomicBool>,
    pub stop_signal: Arc<AtomicBool>,
    pub paused_signal: Arc<AtomicBool>,
    pub stream_res: (usize, usize, u32),
}

impl Clone for OnDemandVideoStream {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            latest_frame: Arc::clone(&self.latest_frame),
            thumb_frame: Arc::clone(&self.thumb_frame),
            status: Arc::clone(&self.status),
            thumb_rev: Arc::clone(&self.thumb_rev),
            is_streaming: Arc::new(AtomicBool::new(false)),
            stop_signal: Arc::new(AtomicBool::new(false)),
            paused_signal: Arc::clone(&self.paused_signal),
            stream_res: self.stream_res,
        }
    }
}

impl OnDemandVideoStream {
    pub fn stop(&self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.is_streaming.store(false, Ordering::Relaxed);
        // Liberar inmediatamente el frame pesado de la RAM cuando deja de reproducirse
        if let Ok(mut lf) = self.latest_frame.lock() {
            *lf = None;
        }
    }

    pub fn ensure_streaming_started(&self, width: usize, height: usize, fps: u32) {
        if self.is_streaming.load(Ordering::Relaxed) {
            return;
        }
        self.is_streaming.store(true, Ordering::Relaxed);
        self.stop_signal.store(false, Ordering::Relaxed);

        let path = self.path.clone();
        let latest = Arc::clone(&self.latest_frame);
        let status = Arc::clone(&self.status);
        let stop_sig = Arc::clone(&self.stop_signal);
        let is_str = Arc::clone(&self.is_streaming);
        let paused_sig = Arc::clone(&self.paused_signal);

        thread::spawn(move || {
            let Some(ffmpeg_bin) = ClipBlock::resolve_ffmpeg_bin() else {
                if let Ok(mut st) = status.lock() {
                    *st = DecodeStatus::MissingFfmpeg;
                }
                is_str.store(false, Ordering::Relaxed);
                return;
            };

            let w = width;
            let h = height;
            let target_fps = fps.clamp(20, 60);
            let vf_arg = format!("scale={}:{},fps={}", w, h, target_fps);
            let frame_bytes = w * h * 4;
            let frame_interval = Duration::from_secs_f32(1.0 / (target_fps as f32));

            if let Ok(mut st) = status.lock() {
                *st = DecodeStatus::StreamingActive(w, h);
            }

            if let Ok(mut child) = silent_command(&ffmpeg_bin)
                .args([
                    "-stream_loop",
                    "-1",
                    "-i",
                    &path.to_string_lossy(),
                    "-vf",
                    &vf_arg,
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "-loglevel",
                    "error",
                    "pipe:1",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdout) = child.stdout.take() {
                    let mut raw_buf = vec![0u8; frame_bytes];
                    while !stop_sig.load(Ordering::Relaxed) {
                        if paused_sig.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(40));
                            continue;
                        }
                        let t0 = Instant::now();
                        if stdout.read_exact(&mut raw_buf).is_ok() {
                            // Conversión lineal directa sin hilos extra de Rayon (ultra bajo CPU)
                            let mut pixels = Vec::with_capacity(w * h);
                            for p in raw_buf.chunks_exact(4) {
                                pixels.push(Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]));
                            }
                            if let Ok(mut lf) = latest.lock() {
                                *lf = Some(ColorImage { size: [w, h], pixels });
                            }
                            let elapsed = t0.elapsed();
                            if elapsed < frame_interval {
                                thread::sleep(frame_interval - elapsed);
                            }
                        } else {
                            break;
                        }
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }

            is_str.store(false, Ordering::Relaxed);
        });
    }
}

#[derive(Clone)]
pub enum ClipSource {
    Empty,
    Generator(GeneratorType),
    StaticImage(PathBuf),
    MovDxv3Stream(OnDemandVideoStream),
}

impl PartialEq for ClipSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ClipSource::Empty, ClipSource::Empty) => true,
            (ClipSource::Generator(a), ClipSource::Generator(b)) => a == b,
            (ClipSource::StaticImage(a), ClipSource::StaticImage(b)) => a == b,
            (ClipSource::MovDxv3Stream(a), ClipSource::MovDxv3Stream(b)) => a.path == b.path,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub struct ClipBlock {
    pub name: String,
    pub codec_tag: String,
    pub source: ClipSource,
    pub speed: f32,
    pub paused: bool,
    pub persistent: bool,
    pub playhead: f32,
    pub duration_secs: f32,
    pub static_frame: Option<ColorImage>,
    pub thumb_revision: usize,
}

impl ClipBlock {
    pub fn empty() -> Self {
        Self {
            name: String::new(),
            codec_tag: "-".into(),
            source: ClipSource::Empty,
            speed: 1.0,
            paused: false,
            persistent: false,
            playhead: 0.0,
            duration_secs: 8.0,
            static_frame: None,
            thumb_revision: 1,
        }
    }

    pub fn from_generator(gen: GeneratorType) -> Self {
        Self {
            name: gen.name().into(),
            codec_tag: "DXV3 GPU Synth (.MOV)".into(),
            source: ClipSource::Generator(gen),
            speed: 1.0,
            paused: false,
            persistent: false,
            playhead: 0.0,
            duration_secs: 10.0,
            static_frame: None,
            thumb_revision: 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.source == ClipSource::Empty && self.static_frame.is_none()
    }

    pub fn stop_stream_if_any(&self) {
        if let ClipSource::MovDxv3Stream(s) = &self.source {
            s.stop();
        }
    }

    pub fn current_thumb_rev(&self) -> usize {
        match &self.source {
            ClipSource::MovDxv3Stream(s) => s.thumb_rev.load(Ordering::Relaxed),
            _ => self.thumb_revision,
        }
    }

    pub fn resolve_ffmpeg_bin() -> Option<String> {
        let candidates = [
            "ffmpeg",
            "ffmpeg.exe",
            "./ffmpeg.exe",
            "./bin/ffmpeg.exe",
            "C:\\ffmpeg\\bin\\ffmpeg.exe",
        ];
        for cand in candidates {
            if silent_command(cand)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
            {
                return Some(cand.to_string());
            }
        }
        if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
            let winget_path = format!("{}\\Microsoft\\WinGet\\Links\\ffmpeg.exe", local_app);
            if Path::new(&winget_path).exists() {
                return Some(winget_path);
            }
        }
        None
    }

    pub fn load_from_path(path: PathBuf, internal_w: usize, internal_h: usize, target_fps: u32) -> Self {
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();

        if ["png", "jpg", "jpeg", "bmp"].contains(&ext.as_str()) {
            if let Ok(img) = image::open(&path) {
                let resized = img
                    .resize_exact(internal_w as u32, internal_h as u32, image::imageops::FilterType::Triangle)
                    .to_rgba8();
                let pixels: Vec<Color32> = resized
                    .chunks_exact(4)
                    .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                    .collect();
                return Self {
                    name: file_name,
                    codec_tag: format!("Textura GPU ({})", ext.to_uppercase()),
                    source: ClipSource::StaticImage(path),
                    speed: 1.0,
                    paused: false,
                    persistent: false,
                    playhead: 0.0,
                    duration_secs: 10.0,
                    static_frame: Some(ColorImage {
                        size: [internal_w, internal_h],
                        pixels,
                    }),
                    thumb_revision: 2,
                };
            }
        }

        let stream = OnDemandVideoStream {
            path: path.clone(),
            latest_frame: Arc::new(Mutex::new(None)),
            thumb_frame: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(DecodeStatus::StandbyThumbReady)),
            thumb_rev: Arc::new(AtomicUsize::new(1)),
            is_streaming: Arc::new(AtomicBool::new(false)),
            stop_signal: Arc::new(AtomicBool::new(false)),
            paused_signal: Arc::new(AtomicBool::new(false)),
            stream_res: (internal_w, internal_h, target_fps),
        };

        // Extraer únicamente 1 fotograma ligero (160x90 = 57 KB) para la miniatura y cerrar FFmpeg inmediatamente (0% CPU en espera)
        let thumb_arc = Arc::clone(&stream.thumb_frame);
        let rev_arc = Arc::clone(&stream.thumb_rev);
        let status_arc = Arc::clone(&stream.status);
        let p_clone = path.clone();

        thread::spawn(move || {
            let Some(ffmpeg_bin) = Self::resolve_ffmpeg_bin() else {
                if let Ok(mut st) = status_arc.lock() {
                    *st = DecodeStatus::MissingFfmpeg;
                }
                rev_arc.fetch_add(1, Ordering::Relaxed);
                return;
            };

            if let Ok(mut child) = silent_command(&ffmpeg_bin)
                .args([
                    "-i",
                    &p_clone.to_string_lossy(),
                    "-vf",
                    "scale=160:90",
                    "-frames:v",
                    "1",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "-loglevel",
                    "error",
                    "pipe:1",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdout) = child.stdout.take() {
                    let mut buf = vec![0u8; THUMB_W * THUMB_H * 4];
                    if stdout.read_exact(&mut buf).is_ok() {
                        let pixels: Vec<Color32> = buf
                            .chunks_exact(4)
                            .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                            .collect();
                        if let Ok(mut tf) = thumb_arc.lock() {
                            *tf = Some(ColorImage {
                                size: [THUMB_W, THUMB_H],
                                pixels,
                            });
                        }
                        rev_arc.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let _ = child.wait();
            }
        });

        let is_mov_dxv = ext == "mov" || ext == "dxv";
        let codec_tag = if is_mov_dxv {
            "DXV3 .MOV On-Demand GPU".to_string()
        } else {
            format!("{} On-Demand Stream", ext.to_uppercase())
        };

        Self {
            name: file_name,
            codec_tag,
            source: ClipSource::MovDxv3Stream(stream),
            speed: 1.0,
            paused: false,
            persistent: false,
            playhead: 0.0,
            duration_secs: 10.0,
            static_frame: None,
            thumb_revision: 1,
        }
    }

    pub fn render_thumb_into(&self, target: &mut [Color32], width: usize, height: usize) {
        if let ClipSource::MovDxv3Stream(stream) = &self.source {
            if let Ok(lock) = stream.thumb_frame.try_lock() {
                if let Some(img) = &*lock {
                    if img.pixels.len() == target.len() {
                        target.copy_from_slice(&img.pixels);
                        return;
                    }
                }
            }
        }
        self.render_into(target, width, height, 0.6, false);
    }

    pub fn render_into(&self, target: &mut [Color32], width: usize, height: usize, time: f32, start_stream_if_video: bool) {
        if let Some(img) = &self.static_frame {
            if img.pixels.len() == target.len() {
                target.copy_from_slice(&img.pixels);
                return;
            }
        }

        let effective_time = if self.paused {
            self.playhead * self.duration_secs
        } else {
            time * self.speed
        };

        match &self.source {
            ClipSource::Empty => {
                target.fill(Color32::BLACK);
            }
            ClipSource::StaticImage(_) => {}
            ClipSource::MovDxv3Stream(stream) => {
                if start_stream_if_video {
                    stream.ensure_streaming_started(width, height, stream.stream_res.2);
                }
                stream.paused_signal.store(self.paused, Ordering::Relaxed);
                if let Ok(lock) = stream.latest_frame.try_lock() {
                    if let Some(frame) = &*lock {
                        if frame.pixels.len() == target.len() {
                            target.copy_from_slice(&frame.pixels);
                            return;
                        }
                    }
                }
                // Si el stream está abriéndose o en preview estático, escalar suavemente la miniatura sin usar CPU pesado
                if let Ok(tlock) = stream.thumb_frame.try_lock() {
                    if let Some(tframe) = &*tlock {
                        let tw = tframe.size[0];
                        let th = tframe.size[1];
                        for y in 0..height {
                            let sy = (y * th) / height;
                            let dst_row = y * width;
                            let src_row = sy * tw;
                            for x in 0..width {
                                let sx = (x * tw) / width;
                                target[dst_row + x] = tframe.pixels[src_row + sx];
                            }
                        }
                        return;
                    }
                }
                let is_missing_ffmpeg = stream
                    .status
                    .try_lock()
                    .map(|s| *s == DecodeStatus::MissingFfmpeg)
                    .unwrap_or(false);
                Self::draw_status_pattern(target, width, height, is_missing_ffmpeg, effective_time);
            }
            ClipSource::Generator(gen) => {
                Self::eval_generator_fast(*gen, target, width, height, effective_time);
            }
        }
    }

    fn draw_status_pattern(target: &mut [Color32], width: usize, height: usize, missing_ffmpeg: bool, t: f32) {
        for y in 0..height {
            let row = y * width;
            for x in 0..width {
                if missing_ffmpeg {
                    let band = (x * 7) / width;
                    target[row + x] = match band {
                        0 => Color32::from_rgb(140, 140, 140),
                        1 => Color32::from_rgb(150, 150, 20),
                        2 => Color32::from_rgb(20, 150, 150),
                        3 => Color32::from_rgb(20, 150, 50),
                        4 => Color32::from_rgb(150, 20, 150),
                        5 => Color32::from_rgb(160, 30, 30),
                        _ => Color32::from_rgb(30, 50, 160),
                    };
                } else {
                    let pulse = ((x as f32 / width as f32 - (t * 0.8).fract()).abs() < 0.12) as u8 * 180;
                    target[row + x] = Color32::from_rgb(15, 25 + pulse / 2, 35 + pulse);
                }
            }
        }
    }

    /// Aproximación rápida de seno sin funciones trigonométricas pesadas (10x más veloz en CPU)
    #[inline(always)]
    fn fast_sin(mut x: f32) -> f32 {
        x = (x * std::f32::consts::FRAC_1_PI * 0.5).fract();
        if x < 0.0 {
            x += 1.0;
        }
        let y = x * 2.0 - 1.0;
        let s = 4.0 * y * (1.0 - y.abs());
        0.225 * (s * s.abs() - s) + s
    }

    /// Generador ultra ligero de un solo paso sin Rayon
    fn eval_generator_fast(gen: GeneratorType, target: &mut [Color32], width: usize, height: usize, t: f32) {
        let inv_w = 2.0 / width as f32;
        let inv_h = 2.0 / height as f32;

        for y in 0..height {
            let ny = (y as f32) * inv_h - 1.0;
            let ny2 = ny * ny;
            let row = y * width;

            for x in 0..width {
                let nx = (x as f32) * inv_w - 1.0;
                target[row + x] = match gen {
                    GeneratorType::NeonTunnel => {
                        let r2 = (nx * nx + ny2).max(0.03);
                        let v = (Self::fast_sin(0.9 / r2 - t * 3.5) * Self::fast_sin(nx * 3.0 + ny * 3.0 + t + 1.57)).max(0.0);
                        Color32::from_rgb((v * 20.0) as u8, (v * 230.0) as u8, (v * 255.0) as u8)
                    }
                    GeneratorType::CyberGrid => {
                        let horizon = ny + 0.2;
                        if horizon > 0.02 {
                            let z = 0.6 / horizon;
                            let gx = (nx * z * 4.0 + t * 0.8).fract().abs();
                            let gz = (z * 3.0 - t * 3.0).fract().abs();
                            if gx < 0.07 || gz < 0.07 {
                                Color32::from_rgb(255, 40, 190)
                            } else {
                                Color32::from_rgb(10, 5, 20)
                            }
                        } else {
                            let glow = (1.0 - horizon.abs() * 3.5).max(0.0);
                            Color32::from_rgb((glow * 120.0) as u8, 0, (glow * 180.0) as u8)
                        }
                    }
                    GeneratorType::SaberPulse => {
                        let s1 = Self::fast_sin(nx * 2.0 + t * 2.0) * 0.4;
                        let s2 = Self::fast_sin(ny * 2.0 - t * 2.5 + 1.57) * 0.4;
                        let beam1 = (1.0 - (ny - s1).abs() * 9.5).max(0.0);
                        let beam2 = (1.0 - (nx - s2).abs() * 9.5).max(0.0);
                        let v = (beam1 + beam2).min(1.0);
                        Color32::from_rgb((v * 40.0) as u8, (v * 255.0) as u8, (v * 140.0) as u8)
                    }
                    GeneratorType::PlasmaWave => {
                        let v1 = Self::fast_sin(nx * 5.0 + t * 2.0);
                        let v2 = Self::fast_sin((nx * nx + ny2) * 5.5 - t * 3.0 + 1.57);
                        let v = ((v1 + v2) * 0.5 + 0.5).clamp(0.0, 1.0);
                        Color32::from_rgb((v * 180.0) as u8, ((1.0 - v) * 90.0) as u8, (v * 255.0) as u8)
                    }
                    GeneratorType::SliceStrobe => {
                        let bar = ((nx * 8.0 + (t * 8.0).floor()) as i32) & 1 == 0;
                        let flash = (Self::fast_sin(t * 12.0) > 0.2) && bar;
                        if flash {
                            Color32::from_rgb(245, 250, 255)
                        } else {
                            Color32::from_rgb(8, 8, 12)
                        }
                    }
                    GeneratorType::FireBorders => {
                        let edge = nx.abs().max(ny.abs());
                        let wave = Self::fast_sin(nx * 8.0 + t * 5.0) * Self::fast_sin(ny * 8.0 - t * 4.0) * 0.07;
                        let intensity = ((edge + wave - 0.65) * 3.5).clamp(0.0, 1.0);
                        Color32::from_rgb(
                            (intensity * 255.0) as u8,
                            (intensity * intensity * 180.0) as u8,
                            (intensity * 30.0) as u8,
                        )
                    }
                };
            }
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct LayerFX {
    pub rgb_shift: f32,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub invert: bool,
    pub strobe: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LayerTransform {
    pub pos_x: f32,
    pub pos_y: f32,
    pub scale: f32,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            pos_x: 0.0,
            pos_y: 0.0,
            scale: 100.0,
        }
    }
}

pub struct Layer {
    pub opacity: f32,
    pub bypassed: bool,
    pub solo: bool,
    pub blend_mode: BlendMode,
    pub bus: BusAssign,
    pub playing_clip: Option<ClipBlock>,
    pub playing_origin: Option<(usize, usize)>,
    pub fx: LayerFX,
    pub transform: LayerTransform,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OutputSlice {
    pub name: String,
    pub send_to_screen: bool,
    pub shape: SliceShape,
    pub source: SliceSource,
    pub preserve_aspect_ratio: bool,
    pub mesh_mode_enabled: bool,
    pub mesh_subdivisions: usize,
    pub corner_points: [[f32; 2]; 4],
    pub mesh_points: Vec<[f32; 2]>,
    pub color_rgb: [u8; 3],
}

impl OutputSlice {
    pub fn color_id(&self) -> Color32 {
        Color32::from_rgb(self.color_rgb[0], self.color_rgb[1], self.color_rgb[2])
    }

    pub fn new_rect(name: &str, source: SliceSource, x: f32, y: f32, w: f32, h: f32, color_rgb: [u8; 3]) -> Self {
        let corners = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
        let mut s = Self {
            name: name.into(),
            send_to_screen: true,
            shape: SliceShape::Rectangle,
            source,
            preserve_aspect_ratio: true,
            mesh_mode_enabled: false,
            mesh_subdivisions: 3,
            corner_points: corners,
            mesh_points: Vec::new(),
            color_rgb,
        };
        s.rebuild_mesh_from_corners();
        s
    }

    pub fn bounding_box(&self) -> [f32; 4] {
        let mut min_x = 1.0f32;
        let mut min_y = 1.0f32;
        let mut max_x = 0.0f32;
        let mut max_y = 0.0f32;
        let pts: &[[f32; 2]] = if self.mesh_mode_enabled && !self.mesh_points.is_empty() {
            &self.mesh_points
        } else {
            &self.corner_points
        };
        for p in pts {
            min_x = min_x.min(p[0]);
            min_y = min_y.min(p[1]);
            max_x = max_x.max(p[0]);
            max_y = max_y.max(p[1]);
        }
        [min_x, min_y, (max_x - min_x).max(0.02), (max_y - min_y).max(0.02)]
    }

    pub fn set_from_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.corner_points = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
        self.rebuild_mesh_from_corners();
    }

    pub fn rebuild_mesh_from_corners(&mut self) {
        let n = self.mesh_subdivisions.max(2);
        let mut pts = Vec::with_capacity(n * n);
        let [tl, tr, br, bl] = self.corner_points;

        for r in 0..n {
            let v = r as f32 / (n - 1) as f32;
            for c in 0..n {
                let u = c as f32 / (n - 1) as f32;
                let top_x = tl[0] * (1.0 - u) + tr[0] * u;
                let top_y = tl[1] * (1.0 - u) + tr[1] * u;
                let bot_x = bl[0] * (1.0 - u) + br[0] * u;
                let bot_y = bl[1] * (1.0 - u) + br[1] * u;

                let px = top_x * (1.0 - v) + bot_x * v;
                let py = top_y * (1.0 - v) + bot_y * v;
                pts.push([px, py]);
            }
        }
        self.mesh_points = pts;
    }

    fn compute_uv_bounds(&self) -> [f32; 4] {
        if !self.preserve_aspect_ratio {
            return [0.0, 0.0, 1.0, 1.0];
        }
        let [_, _, bw, bh] = self.bounding_box();
        let slice_aspect = (bw * 16.0) / (bh * 9.0).max(0.001);
        let video_aspect = 16.0 / 9.0;
        let ratio = slice_aspect / video_aspect;

        if ratio > 1.0 {
            let vis_h = (1.0 / ratio).clamp(0.05, 1.0);
            [0.0, (1.0 - vis_h) * 0.5, 1.0, vis_h]
        } else {
            let vis_w = ratio.clamp(0.05, 1.0);
            [(1.0 - vis_w) * 0.5, 0.0, vis_w, 1.0]
        }
    }

    fn eval_bilinear_pos(&self, u: f32, v: f32) -> [f32; 2] {
        let [tl, tr, br, bl] = self.corner_points;
        let top_x = tl[0] * (1.0 - u) + tr[0] * u;
        let top_y = tl[1] * (1.0 - u) + tr[1] * u;
        let bot_x = bl[0] * (1.0 - u) + br[0] * u;
        let bot_y = bl[1] * (1.0 - u) + br[1] * u;
        [top_x * (1.0 - v) + bot_x * v, top_y * (1.0 - v) + bot_y * v]
    }

    pub fn build_geometry(&self) -> (Vec<([f32; 2], [f32; 2])>, Vec<[usize; 3]>) {
        let [ux, uy, uw, uh] = self.compute_uv_bounds();

        if self.mesh_mode_enabled && self.mesh_points.len() >= 4 {
            let n = self.mesh_subdivisions.max(2);
            if self.mesh_points.len() == n * n {
                let mut verts = Vec::with_capacity(n * n);
                let mut tris = Vec::with_capacity((n - 1) * (n - 1) * 2);

                for r in 0..n {
                    let v = r as f32 / (n - 1) as f32;
                    for c in 0..n {
                        let u = c as f32 / (n - 1) as f32;
                        let pos = self.mesh_points[r * n + c];
                        let uv = [ux + u * uw, uy + v * uh];
                        verts.push((pos, uv));
                    }
                }

                for r in 0..(n - 1) {
                    for c in 0..(n - 1) {
                        let i0 = r * n + c;
                        let i1 = i0 + 1;
                        let i2 = (r + 1) * n + (c + 1);
                        let i3 = (r + 1) * n + c;
                        tris.push([i0, i1, i2]);
                        tris.push([i0, i2, i3]);
                    }
                }
                return (verts, tris);
            }
        }

        match self.shape {
            SliceShape::Rectangle => {
                let verts = vec![
                    (self.corner_points[0], [ux, uy]),
                    (self.corner_points[1], [ux + uw, uy]),
                    (self.corner_points[2], [ux + uw, uy + uh]),
                    (self.corner_points[3], [ux, uy + uh]),
                ];
                let tris = vec![[0, 1, 2], [0, 2, 3]];
                (verts, tris)
            }
            SliceShape::Triangle => {
                let top_mid = self.eval_bilinear_pos(0.5, 0.0);
                let bot_right = self.eval_bilinear_pos(1.0, 1.0);
                let bot_left = self.eval_bilinear_pos(0.0, 1.0);
                let verts = vec![
                    (top_mid, [ux + uw * 0.5, uy]),
                    (bot_right, [ux + uw, uy + uh]),
                    (bot_left, [ux, uy + uh]),
                ];
                let tris = vec![[0, 1, 2]];
                (verts, tris)
            }
            SliceShape::Circle | SliceShape::Hexagon => {
                let segments = if self.shape == SliceShape::Hexagon { 6 } else { 32 };
                let center_pos = self.eval_bilinear_pos(0.5, 0.5);
                let center_uv = [ux + uw * 0.5, uy + uh * 0.5];

                let mut verts = Vec::with_capacity(segments + 1);
                let mut tris = Vec::with_capacity(segments);
                verts.push((center_pos, center_uv));

                for i in 0..segments {
                    let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
                    let u = 0.5 + 0.5 * angle.cos();
                    let v = 0.5 + 0.5 * angle.sin();
                    let pos = self.eval_bilinear_pos(u, v);
                    let uv = [(ux + u * uw).clamp(0.0, 1.0), (uy + v * uh).clamp(0.0, 1.0)];
                    verts.push((pos, uv));
                }

                for i in 0..segments {
                    let next = if i + 1 == segments { 1 } else { i + 2 };
                    tris.push([0, i + 1, next]);
                }
                (verts, tris)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Dxv3ConvertProfile {
    NormalNoAlpha,
    HighWithAlpha,
}

impl Dxv3ConvertProfile {
    pub fn label(&self) -> &'static str {
        match self {
            Dxv3ConvertProfile::NormalNoAlpha => "DXV3 Normal Quality (.MOV - Sin Alpha)",
            Dxv3ConvertProfile::HighWithAlpha => "DXV3 High Quality With Alpha (.MOV - RGBA)",
        }
    }
}

pub struct VideoConverterState {
    pub input_path: Option<PathBuf>,
    pub output_path: Option<PathBuf>,
    pub profile: Dxv3ConvertProfile,
    pub target_fps: u32,
    pub is_converting: Arc<AtomicBool>,
    pub status_message: Arc<Mutex<String>>,
    pub last_converted_file: Arc<Mutex<Option<PathBuf>>>,
}

impl Default for VideoConverterState {
    fn default() -> Self {
        Self {
            input_path: None,
            output_path: None,
            profile: Dxv3ConvertProfile::NormalNoAlpha,
            target_fps: 60,
            is_converting: Arc::new(AtomicBool::new(false)),
            status_message: Arc::new(Mutex::new("Selecciona un archivo de video para transcodificarlo a .MOV (DXV3).".into())),
            last_converted_file: Arc::new(Mutex::new(None)),
        }
    }
}

pub struct Deck {
    pub name: String,
    pub layer_names: Vec<String>,
    pub grid: Vec<Vec<ClipBlock>>,
}

/// El escalado final de Slices y bordes se hace siempre a 1080p/4K nativo en el shader WGPU,
/// mientras que el muestreo interno usa un pool ligero para no saturar el bus PCIe ni la RAM.
pub struct RenderBufferPool {
    pub width: usize,
    pub height: usize,
    pub master_buf: Vec<Color32>,
    pub layer_buffers: Vec<Vec<Color32>>,
    pub fx_scratch: Vec<Color32>,
    pub preview_buf: Vec<Color32>,
}

impl RenderBufferPool {
    pub fn new(width: usize, height: usize, num_layers: usize) -> Self {
        let len = width * height;
        Self {
            width,
            height,
            master_buf: vec![Color32::BLACK; len],
            layer_buffers: vec![vec![Color32::BLACK; len]; num_layers],
            fx_scratch: vec![Color32::BLACK; len],
            preview_buf: vec![Color32::BLACK; PREVIEW_W * PREVIEW_H],
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let len = width * height;
        self.master_buf = vec![Color32::BLACK; len];
        for lb in &mut self.layer_buffers {
            *lb = vec![Color32::BLACK; len];
        }
        self.fx_scratch = vec![Color32::BLACK; len];
    }
}

#[derive(Serialize, Deserialize)]
pub struct SavedClip {
    pub kind: String,
    pub value: String,
    pub speed: f32,
    pub persistent: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SavedDeck {
    pub name: String,
    pub layer_names: Vec<String>,
    pub grid: Vec<Vec<SavedClip>>,
}

#[derive(Serialize, Deserialize)]
pub struct SavedLayerState {
    pub opacity: f32,
    pub bypassed: bool,
    pub solo: bool,
    pub blend_mode: BlendMode,
    pub bus: BusAssign,
    pub fx: LayerFX,
    pub transform: LayerTransform,
}

#[derive(Serialize, Deserialize)]
pub struct SavedSession {
    pub output_resolution: (u32, u32),
    pub target_fps: u32,
    pub bpm: f32,
    pub master_opacity: f32,
    pub current_deck: usize,
    pub decks: Vec<SavedDeck>,
    pub layers: Vec<SavedLayerState>,
    pub slices: Vec<OutputSlice>,
}

pub struct VjEngine {
    pub layers: Vec<Layer>,
    pub decks: Vec<Deck>,
    pub current_deck: usize,
    pub num_columns: usize,
    pub master_opacity: f32,
    pub master_speed: f32,
    pub crossfader: f32,
    pub bpm: f32,
    pub slices: Vec<OutputSlice>,
    pub selected_slice: usize,
    pub preview_target: Option<(usize, usize)>,
    pub monitors: Vec<WindowsMonitor>,
    pub selected_monitor_idx: usize,
    pub output_resolution: (u32, u32), // Resolución física del Canvas WGPU (1920x1080 / 4K)
    pub target_fps: u32,
    pub pool: RenderBufferPool,
    pub ffmpeg_installing: Arc<AtomicBool>,
    pub converter: VideoConverterState,
}

impl VjEngine {
    /// Calcula la resolución óptima de muestreo interno que la GPU escala por hardware al tamaño del Canvas
    pub fn optimal_internal_dims(canvas_res: (u32, u32)) -> (usize, usize) {
        match canvas_res.0 {
            0..=1280 => (640, 360),
            1281..=1920 => (800, 450),
            _ => (960, 540),
        }
    }

    pub fn new() -> Self {
        let num_layers = 3;
        let num_columns = 10;
        let default_res = (1920, 1080);
        let (iw, ih) = Self::optimal_internal_dims(default_res);

        let mut grid_deck1 = vec![vec![ClipBlock::empty(); num_columns]; num_layers];
        grid_deck1[0][0] = ClipBlock::from_generator(GeneratorType::FireBorders);
        grid_deck1[0][1] = ClipBlock::from_generator(GeneratorType::SaberPulse);
        grid_deck1[0][2] = ClipBlock::from_generator(GeneratorType::SliceStrobe);

        grid_deck1[1][0] = ClipBlock::from_generator(GeneratorType::NeonTunnel);
        grid_deck1[1][1] = ClipBlock::from_generator(GeneratorType::CyberGrid);
        grid_deck1[1][3] = ClipBlock::from_generator(GeneratorType::PlasmaWave);

        grid_deck1[2][0] = ClipBlock::from_generator(GeneratorType::PlasmaWave);
        grid_deck1[2][1] = ClipBlock::from_generator(GeneratorType::NeonTunnel);

        let mut grid_deck2 = vec![vec![ClipBlock::empty(); num_columns]; num_layers];
        grid_deck2[0][0] = ClipBlock::from_generator(GeneratorType::SliceStrobe);
        grid_deck2[1][0] = ClipBlock::from_generator(GeneratorType::CyberGrid);
        grid_deck2[2][0] = ClipBlock::from_generator(GeneratorType::FireBorders);

        let grid_deck3 = vec![vec![ClipBlock::empty(); num_columns]; num_layers];

        let init_clip_l0 = grid_deck1[0][0].clone();
        let init_clip_l1 = grid_deck1[1][0].clone();
        let init_clip_l2 = grid_deck1[2][0].clone();

        let monitors = WindowsMonitor::enumerate_windows_displays();

        Self {
            layers: vec![
                Layer {
                    opacity: 1.0,
                    bypassed: false,
                    solo: false,
                    blend_mode: BlendMode::Add,
                    bus: BusAssign::Through,
                    playing_clip: Some(init_clip_l0),
                    playing_origin: Some((0, 0)),
                    fx: LayerFX::default(),
                    transform: LayerTransform::default(),
                },
                Layer {
                    opacity: 1.0,
                    bypassed: false,
                    solo: false,
                    blend_mode: BlendMode::Alpha,
                    bus: BusAssign::CrossA,
                    playing_clip: Some(init_clip_l1),
                    playing_origin: Some((0, 0)),
                    fx: LayerFX::default(),
                    transform: LayerTransform::default(),
                },
                Layer {
                    opacity: 1.0,
                    bypassed: false,
                    solo: false,
                    blend_mode: BlendMode::Alpha,
                    bus: BusAssign::CrossB,
                    playing_clip: Some(init_clip_l2),
                    playing_origin: Some((0, 0)),
                    fx: LayerFX::default(),
                    transform: LayerTransform::default(),
                },
            ],
            decks: vec![
                Deck {
                    name: "Fire Borders (Deck 1)".into(),
                    layer_names: vec![
                        "CAPA 3 · FX / TOP".into(),
                        "CAPA 2 · VIDEOS".into(),
                        "CAPA 1 · FONDO".into(),
                    ],
                    grid: grid_deck1,
                },
                Deck {
                    name: "Cyber Set (Deck 2)".into(),
                    layer_names: vec![
                        "CAPA 3 · ESTROBOS".into(),
                        "CAPA 2 · ESCENARIO LED".into(),
                        "CAPA 1 · BASE CYBER".into(),
                    ],
                    grid: grid_deck2,
                },
                Deck {
                    name: "Visuals Aux (Deck 3)".into(),
                    layer_names: vec![
                        "CAPA 3 · LOGOS".into(),
                        "CAPA 2 · CLIPS AUX".into(),
                        "CAPA 1 · AMBIENTE".into(),
                    ],
                    grid: grid_deck3,
                },
            ],
            current_deck: 0,
            num_columns,
            master_opacity: 1.0,
            master_speed: 1.0,
            crossfader: 0.0,
            bpm: 124.0,
            slices: vec![
                OutputSlice::new_rect(
                    "Pantalla Central LED",
                    SliceSource::Layer(1),
                    0.22,
                    0.15,
                    0.56,
                    0.70,
                    [0, 235, 175],
                ),
                {
                    let mut s = OutputSlice::new_rect(
                        "Domo Circular Sup",
                        SliceSource::Layer(0),
                        0.40,
                        0.02,
                        0.20,
                        0.26,
                        [0, 195, 255],
                    );
                    s.shape = SliceShape::Circle;
                    s
                },
                OutputSlice::new_rect(
                    "Totem Izquierdo",
                    SliceSource::Layer(2),
                    0.03,
                    0.10,
                    0.15,
                    0.80,
                    [255, 165, 0],
                ),
                OutputSlice::new_rect(
                    "Totem Derecho",
                    SliceSource::Layer(2),
                    0.82,
                    0.10,
                    0.15,
                    0.80,
                    [255, 65, 165],
                ),
            ],
            selected_slice: 0,
            preview_target: Some((1, 0)),
            monitors,
            selected_monitor_idx: 0,
            output_resolution: default_res,
            target_fps: 60,
            pool: RenderBufferPool::new(iw, ih, num_layers),
            ffmpeg_installing: Arc::new(AtomicBool::new(false)),
            converter: VideoConverterState::default(),
        }
    }

    pub fn current_layer_names(&self) -> Vec<String> {
        self.decks
            .get(self.current_deck)
            .map(|d| d.layer_names.clone())
            .unwrap_or_else(|| vec!["CAPA 3".into(), "CAPA 2".into(), "CAPA 1".into()])
    }

    pub fn set_output_resolution_and_fps(&mut self, w: u32, h: u32, fps: u32) {
        self.output_resolution = (w, h);
        self.target_fps = fps;
        let (iw, ih) = Self::optimal_internal_dims((w, h));
        self.pool.resize(iw, ih);

        for layer in &mut self.layers {
            if let Some(block) = &mut layer.playing_clip {
                block.stop_stream_if_any();
            }
        }
    }

    pub fn set_clip_persistent(&mut self, layer_idx: usize, col_idx: usize, persistent: bool) {
        let cur = self.current_deck;
        self.decks[cur].grid[layer_idx][col_idx].persistent = persistent;
        if persistent {
            let cloned_block = self.decks[cur].grid[layer_idx][col_idx].clone();
            for d_idx in 0..self.decks.len() {
                if d_idx != cur {
                    self.decks[d_idx].grid[layer_idx][col_idx] = cloned_block.clone();
                }
            }
        }
    }

    pub fn delete_deck(&mut self, deck_idx: usize) {
        if self.decks.len() <= 1 {
            let cols = self.num_columns;
            let rows = self.layers.len();
            self.decks[0].grid = vec![vec![ClipBlock::empty(); cols]; rows];
            return;
        }
        self.decks.remove(deck_idx);
        if self.current_deck >= self.decks.len() {
            self.current_deck = self.decks.len() - 1;
        }
    }

    pub fn trigger_clip(&mut self, layer_idx: usize, col_idx: usize) {
        let deck_idx = self.current_deck;
        let block = self.decks[deck_idx].grid[layer_idx][col_idx].clone();
        if !block.is_empty() {
            if let Some(layer) = self.layers.get_mut(layer_idx) {
                // Detener inmediatamente el stream de video anterior para liberar CPU y RAM
                if let Some(old_clip) = &layer.playing_clip {
                    old_clip.stop_stream_if_any();
                }
                layer.playing_clip = Some(block);
                layer.playing_origin = Some((deck_idx, col_idx));
            }
        }
    }

    pub fn trigger_column(&mut self, col_idx: usize) {
        for l_idx in 0..self.layers.len() {
            self.trigger_clip(l_idx, col_idx);
        }
    }

    pub fn save_session_to_file(&self, path: &Path) -> Result<(), String> {
        let mut saved_decks = Vec::new();
        for d in &self.decks {
            let mut s_grid = Vec::new();
            for row in &d.grid {
                let mut s_row = Vec::new();
                for cell in row {
                    let (kind, val) = match &cell.source {
                        ClipSource::Empty => ("empty".to_string(), String::new()),
                        ClipSource::Generator(g) => ("gen".to_string(), format!("{:?}", g)),
                        ClipSource::StaticImage(p) => ("img".to_string(), p.to_string_lossy().to_string()),
                        ClipSource::MovDxv3Stream(s) => ("mov".to_string(), s.path.to_string_lossy().to_string()),
                    };
                    s_row.push(SavedClip {
                        kind,
                        value: val,
                        speed: cell.speed,
                        persistent: cell.persistent,
                    });
                }
                s_grid.push(s_row);
            }
            saved_decks.push(SavedDeck {
                name: d.name.clone(),
                layer_names: d.layer_names.clone(),
                grid: s_grid,
            });
        }

        let saved_layers: Vec<SavedLayerState> = self
            .layers
            .iter()
            .map(|l| SavedLayerState {
                opacity: l.opacity,
                bypassed: l.bypassed,
                solo: l.solo,
                blend_mode: l.blend_mode,
                bus: l.bus,
                fx: l.fx.clone(),
                transform: l.transform.clone(),
            })
            .collect();

        let session = SavedSession {
            output_resolution: self.output_resolution,
            target_fps: self.target_fps,
            bpm: self.bpm,
            master_opacity: self.master_opacity,
            current_deck: self.current_deck,
            decks: saved_decks,
            layers: saved_layers,
            slices: self.slices.clone(),
        };

        let json = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn load_session_from_file(&mut self, path: &Path) -> Result<(), String> {
        let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let session: SavedSession = serde_json::from_str(&data).map_err(|e| e.to_string())?;

        self.set_output_resolution_and_fps(
            session.output_resolution.0,
            session.output_resolution.1,
            session.target_fps,
        );
        self.bpm = session.bpm;
        self.master_opacity = session.master_opacity;
        self.slices = session.slices;
        self.selected_slice = 0;

        let (iw, ih) = (self.pool.width, self.pool.height);
        let fps = self.target_fps;

        let mut loaded_decks = Vec::new();
        for sd in session.decks {
            let mut grid = Vec::new();
            for s_row in sd.grid {
                let mut row = Vec::new();
                for sc in s_row {
                    let mut block = match sc.kind.as_str() {
                        "gen" => {
                            let g = match sc.value.as_str() {
                                "CyberGrid" => GeneratorType::CyberGrid,
                                "SaberPulse" => GeneratorType::SaberPulse,
                                "PlasmaWave" => GeneratorType::PlasmaWave,
                                "SliceStrobe" => GeneratorType::SliceStrobe,
                                "FireBorders" => GeneratorType::FireBorders,
                                _ => GeneratorType::NeonTunnel,
                            };
                            ClipBlock::from_generator(g)
                        }
                        "img" | "mov" => {
                            let p = PathBuf::from(&sc.value);
                            if p.exists() {
                                ClipBlock::load_from_path(p, iw, ih, fps)
                            } else {
                                ClipBlock::empty()
                            }
                        }
                        _ => ClipBlock::empty(),
                    };
                    block.speed = sc.speed;
                    block.persistent = sc.persistent;
                    row.push(block);
                }
                grid.push(row);
            }
            loaded_decks.push(Deck {
                name: sd.name,
                layer_names: sd.layer_names,
                grid,
            });
        }

        if !loaded_decks.is_empty() {
            self.decks = loaded_decks;
            self.current_deck = session.current_deck.min(self.decks.len() - 1);
        }

        for (i, sl) in session.layers.into_iter().enumerate() {
            if let Some(layer) = self.layers.get_mut(i) {
                layer.opacity = sl.opacity;
                layer.bypassed = sl.bypassed;
                layer.solo = sl.solo;
                layer.blend_mode = sl.blend_mode;
                layer.bus = sl.bus;
                layer.fx = sl.fx;
                layer.transform = sl.transform;
            }
        }
        Ok(())
    }

    pub fn update_playheads(&mut self, dt: f32) {
        let bpm_factor = (self.bpm / 120.0).clamp(0.25, 4.0);
        let master_spd = self.master_speed * bpm_factor;
        for l_idx in 0..self.layers.len() {
            let origin = self.layers[l_idx].playing_origin;
            if let Some(playing) = &mut self.layers[l_idx].playing_clip {
                if !playing.paused && playing.duration_secs > 0.1 {
                    playing.playhead =
                        (playing.playhead + (dt * playing.speed * master_spd) / playing.duration_secs).fract();
                }
                let current_ph = playing.playhead;
                if let Some((d_idx, c_idx)) = origin {
                    if let Some(deck) = self.decks.get_mut(d_idx) {
                        if let Some(cell) = deck.grid.get_mut(l_idx).and_then(|r| r.get_mut(c_idx)) {
                            cell.playhead = current_ph;
                        }
                    }
                }
            }
        }
    }

    pub fn render_all_layers_and_master(&mut self, time: f32) {
        let w = self.pool.width;
        let h = self.pool.height;
        self.pool.master_buf.fill(Color32::BLACK);

        let any_solo = self.layers.iter().any(|l| l.solo);
        let bpm_factor = (self.bpm / 120.0).clamp(0.25, 4.0);
        let t = time * self.master_speed * bpm_factor;

        for layer_idx in (0..self.layers.len()).rev() {
            let layer = &self.layers[layer_idx];
            if layer.bypassed || (any_solo && !layer.solo) || layer.opacity <= 0.01 || layer.playing_clip.is_none() {
                continue;
            }

            if let Some(block) = &layer.playing_clip {
                if !block.is_empty() {
                    block.render_into(&mut self.pool.layer_buffers[layer_idx], w, h, t, true);
                    Self::apply_fx_and_transform_in_place(
                        &mut self.pool.layer_buffers[layer_idx],
                        &mut self.pool.fx_scratch,
                        w,
                        h,
                        &layer.fx,
                        &layer.transform,
                        t,
                    );
                }
            }

            let cross_gain = match layer.bus {
                BusAssign::Through => 1.0,
                BusAssign::CrossA => (1.0 - self.crossfader).clamp(0.0, 1.0),
                BusAssign::CrossB => self.crossfader.clamp(0.0, 1.0),
            };
            let op = (layer.opacity * self.master_opacity * cross_gain).clamp(0.0, 1.0);
            if op <= 0.01 {
                continue;
            }

            let op_u32 = (op * 256.0) as u32;
            let inv_op_u32 = 256 - op_u32;
            let blend = layer.blend_mode;
            let src_buf = &self.pool.layer_buffers[layer_idx];

            for (dst, src) in self.pool.master_buf.iter_mut().zip(src_buf.iter()) {
                let (dr, dg, db) = (dst.r() as u32, dst.g() as u32, dst.b() as u32);
                let (sr, sg, sb) = (src.r() as u32, src.g() as u32, src.b() as u32);

                let (r, g, b) = match blend {
                    BlendMode::Alpha => (
                        ((dr * inv_op_u32 + sr * op_u32) >> 8) as u8,
                        ((dg * inv_op_u32 + sg * op_u32) >> 8) as u8,
                        ((db * inv_op_u32 + sb * op_u32) >> 8) as u8,
                    ),
                    BlendMode::Add => (
                        (dr + ((sr * op_u32) >> 8)).min(255) as u8,
                        (dg + ((sg * op_u32) >> 8)).min(255) as u8,
                        (db + ((sb * op_u32) >> 8)).min(255) as u8,
                    ),
                    BlendMode::Multiply => {
                        let mr = (dr * sr) >> 8;
                        let mg = (dg * sg) >> 8;
                        let mb = (db * sb) >> 8;
                        (
                            ((dr * inv_op_u32 + mr * op_u32) >> 8) as u8,
                            ((dg * inv_op_u32 + mg * op_u32) >> 8) as u8,
                            ((db * inv_op_u32 + mb * op_u32) >> 8) as u8,
                        )
                    }
                    BlendMode::Screen => {
                        let scr_r = 255 - (((255 - dr) * (255 - sr)) >> 8);
                        let scr_g = 255 - (((255 - dg) * (255 - sg)) >> 8);
                        let scr_b = 255 - (((255 - db) * (255 - sb)) >> 8);
                        (
                            ((dr * inv_op_u32 + scr_r * op_u32) >> 8) as u8,
                            ((dg * inv_op_u32 + scr_g * op_u32) >> 8) as u8,
                            ((db * inv_op_u32 + scr_b * op_u32) >> 8) as u8,
                        )
                    }
                    BlendMode::Lighten => (
                        dr.max((sr * op_u32) >> 8) as u8,
                        dg.max((sg * op_u32) >> 8) as u8,
                        db.max((sb * op_u32) >> 8) as u8,
                    ),
                };
                *dst = Color32::from_rgb(r, g, b);
            }
        }
    }

    fn apply_fx_and_transform_in_place(
        buf: &mut [Color32],
        scratch: &mut [Color32],
        w: usize,
        h: usize,
        fx: &LayerFX,
        transform: &LayerTransform,
        time: f32,
    ) {
        if (transform.scale - 100.0).abs() > 0.5 || transform.pos_x.abs() > 0.5 || transform.pos_y.abs() > 0.5 {
            scratch.copy_from_slice(buf);
            buf.fill(Color32::BLACK);
            let scale_f = (transform.scale / 100.0).max(0.1);
            let inv_scale = 1.0 / scale_f;
            let cx = w as f32 * 0.5;
            let cy = h as f32 * 0.5;

            for y in 0..h {
                let sy = ((y as f32 - cy - transform.pos_y) * inv_scale + cy) as i32;
                if sy >= 0 && sy < h as i32 {
                    let dst_row = y * w;
                    let src_row_offset = (sy as usize) * w;
                    for x in 0..w {
                        let sx = ((x as f32 - cx - transform.pos_x) * inv_scale + cx) as i32;
                        if sx >= 0 && sx < w as i32 {
                            buf[dst_row + x] = scratch[src_row_offset + sx as usize];
                        }
                    }
                }
            }
        }

        if fx.mirror_x {
            for y in 0..h {
                let row = y * w;
                for x in 0..(w / 2) {
                    buf[row + (w - 1 - x)] = buf[row + x];
                }
            }
        }
        if fx.mirror_y {
            for y in 0..(h / 2) {
                let top_row = y * w;
                let bot_row = (h - 1 - y) * w;
                for x in 0..w {
                    buf[bot_row + x] = buf[top_row + x];
                }
            }
        }
        if fx.rgb_shift > 0.01 {
            let shift = (fx.rgb_shift * 24.0) as usize;
            scratch.copy_from_slice(buf);
            for y in 0..h {
                let row = y * w;
                for x in 0..w {
                    let r_x = (x + shift).min(w - 1);
                    let b_x = x.saturating_sub(shift);
                    let r = scratch[row + r_x].r();
                    let g = scratch[row + x].g();
                    let b = scratch[row + b_x].b();
                    buf[row + x] = Color32::from_rgb(r, g, b);
                }
            }
        }
        if fx.invert {
            for p in buf.iter_mut() {
                *p = Color32::from_rgb(255 - p.r(), 255 - p.g(), 255 - p.b());
            }
        }
        if fx.strobe > 0.05 {
            let on = (ClipBlock::fast_sin(time * (4.0 + fx.strobe * 25.0)) > 0.0);
            if !on {
                buf.fill(Color32::BLACK);
            }
        }
    }
}
