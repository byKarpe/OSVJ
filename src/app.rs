use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke, TextureHandle,
    TextureId, TextureOptions, Vec2, ViewportCommand,
};
use crate::engine::{
    silent_command, BlendMode, BusAssign, ClipBlock, ClipSource, DecodeStatus, Dxv3ConvertProfile, GeneratorType,
    OutputSlice, SliceShape, SliceSource, VjEngine, WindowsMonitor, PREVIEW_H, PREVIEW_W, THUMB_H, THUMB_W,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, System};

pub struct SystemTelemetry {
    sys: System,
    pid: Pid,
    last_refresh: Instant,
    pub cpu_usage: f32,
    pub ram_used_mb: f32,
    pub total_ram_pct: f32,
    pub vram_est_mb: f32,
    pub fps: f32,
    frame_counter: u32,
}

impl SystemTelemetry {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu();
        sys.refresh_memory();
        let pid = sysinfo::get_current_pid().unwrap_or(Pid::from(0));
        Self {
            sys,
            pid,
            last_refresh: Instant::now(),
            cpu_usage: 3.2,
            ram_used_mb: 58.0,
            total_ram_pct: 24.0,
            vram_est_mb: 78.0,
            fps: 60.0,
            frame_counter: 0,
        }
    }

    pub fn tick_render_frame(&mut self, active_textures: usize, internal_dims: (usize, usize)) {
        self.frame_counter += 1;
        let elapsed = self.last_refresh.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            self.fps = (self.frame_counter as f32 / elapsed).clamp(1.0, 240.0);
            self.frame_counter = 0;
            self.last_refresh = Instant::now();

            self.sys.refresh_cpu();
            self.sys.refresh_memory();
            self.sys.refresh_process(self.pid);

            self.cpu_usage = self.sys.global_cpu_info().cpu_usage();
            if let Some(proc_info) = self.sys.process(self.pid) {
                self.ram_used_mb = proc_info.memory() as f32 / (1024.0 * 1024.0);
            }
            let total_mem = self.sys.total_memory().max(1) as f32;
            let used_mem = self.sys.used_memory() as f32;
            self.total_ram_pct = (used_mem / total_mem) * 100.0;
            let mb_per_tex = (internal_dims.0 as f32 * internal_dims.1 as f32 * 4.0) / (1024.0 * 1024.0);
            self.vram_est_mb = 48.0 + (4.0 * mb_per_tex) + (active_textures as f32 * 0.25);
        }
    }
}

#[derive(Clone, Copy)]
pub enum DragHandle {
    Corner(usize, usize),
    MeshNode(usize, usize),
    WholeSlice(usize),
}

#[derive(PartialEq, Clone, Copy)]
pub enum CenterWorkspaceView {
    ClipMatrixGrid,
    SliceDesignerView,
    Dxv3ConverterView,
    DisplayRoutingView,
}

pub struct VjApp {
    pub engine: VjEngine,
    pub telemetry: SystemTelemetry,
    pub start_time: Instant,
    pub last_render_tick: Instant,
    pub selected_layer: usize,
    pub center_view: CenterWorkspaceView,
    pub show_splash_screen: bool,
    pub splash_opened_manually: bool,
    pub show_slice_wireframes_on_liveview: bool,
    pub maximized_monitor: Option<usize>,
    pub monitor_height_scale: f32,
    pub master_tex: Option<TextureHandle>,
    pub layer_textures: Vec<Option<TextureHandle>>,
    pub preview_tex: Option<TextureHandle>,
    pub thumb_textures: Vec<Vec<Option<TextureHandle>>>,
    pub thumb_uploaded_rev: Vec<Vec<usize>>,
    pub thumb_scratch: Vec<Color32>,
    pub active_drag: Option<DragHandle>,
    pub has_ffmpeg_cached: bool,
    pub last_applied_monitor: usize,
    pub tap_times: Vec<Instant>,
    pub current_session_path: Option<PathBuf>,
    pub last_autobackup_instant: Instant,
    pub backup_indicator_timer: f32,
    pub show_exit_modal: bool,
    pub backup_before_exit: bool,
    pub allow_immediate_close: bool,
}

impl VjApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let engine = VjEngine::new();
        let rows = engine.layers.len();
        let cols = engine.num_columns;
        let now = Instant::now();
        let has_ff = ClipBlock::resolve_ffmpeg_bin().is_some();

        Self {
            engine,
            telemetry: SystemTelemetry::new(),
            start_time: now,
            last_render_tick: now,
            selected_layer: 1,
            center_view: CenterWorkspaceView::ClipMatrixGrid,
            show_splash_screen: true,
            splash_opened_manually: false,
            show_slice_wireframes_on_liveview: true,
            maximized_monitor: None,
            monitor_height_scale: 1.0,
            master_tex: None,
            layer_textures: vec![None; rows],
            preview_tex: None,
            thumb_textures: vec![vec![None; cols]; rows],
            thumb_uploaded_rev: vec![vec![0; cols]; rows],
            thumb_scratch: vec![Color32::BLACK; THUMB_W * THUMB_H],
            active_drag: None,
            has_ffmpeg_cached: has_ff,
            last_applied_monitor: 0,
            tap_times: Vec::new(),
            current_session_path: None,
            last_autobackup_instant: now,
            backup_indicator_timer: 0.0,
            show_exit_modal: false,
            backup_before_exit: true,
            allow_immediate_close: false,
        }
    }

    fn perform_backup_now(&mut self, custom_path: Option<PathBuf>) {
        let target = custom_path.unwrap_or_else(|| PathBuf::from("osvj_autobackup.osvj"));
        let _ = self.engine.save_session_to_file(&target);
        self.last_autobackup_instant = Instant::now();
        self.backup_indicator_timer = 3.5;
    }

    fn register_bpm_tap(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.tap_times.last() {
            if now.duration_since(*last).as_secs_f32() > 2.3 {
                self.tap_times.clear();
            }
        }
        self.tap_times.push(now);
        if self.tap_times.len() > 8 {
            self.tap_times.remove(0);
        }
        if self.tap_times.len() >= 2 {
            let mut sum_secs = 0.0f32;
            for pair in self.tap_times.windows(2) {
                sum_secs += pair[1].duration_since(pair[0]).as_secs_f32();
            }
            let avg_secs = sum_secs / (self.tap_times.len() - 1) as f32;
            if avg_secs > 0.15 {
                let calc_bpm = (60.0 / avg_secs).clamp(40.0, 240.0);
                self.engine.bpm = (calc_bpm * 10.0).round() / 10.0;
            }
        }
    }

    fn paint_osvj_vector_logo(painter: &egui::Painter, center: Pos2, radius: f32, time: f32) {
        let pulse = (time * 3.2).sin() * 0.06 + 1.0;
        let r_outer = radius * pulse;

        painter.circle_filled(center, r_outer * 1.18, Color32::from_rgba_unmultiplied(0, 235, 175, 18));
        painter.circle_stroke(
            center,
            r_outer * 1.05,
            Stroke::new(1.5, Color32::from_rgba_unmultiplied(0, 220, 255, 90)),
        );

        let mut hex_pts = Vec::with_capacity(6);
        for i in 0..6 {
            let a = (i as f32 / 6.0) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            hex_pts.push(Pos2::new(center.x + a.cos() * r_outer, center.y + a.sin() * r_outer));
        }
        painter.add(Shape::convex_polygon(
            hex_pts,
            Color32::from_rgb(14, 18, 24),
            Stroke::new(2.5, Color32::from_rgb(0, 240, 175)),
        ));

        let p_top = Pos2::new(center.x - r_outer * 0.22, center.y - r_outer * 0.48);
        let p_bot = Pos2::new(center.x - r_outer * 0.22, center.y + r_outer * 0.48);
        let p_tip = Pos2::new(center.x + r_outer * 0.56, center.y);

        painter.add(Shape::convex_polygon(
            vec![p_top, p_tip, p_bot],
            Color32::from_rgba_unmultiplied(0, 230, 170, 210),
            Stroke::new(1.5, Color32::WHITE),
        ));

        let s_top = Pos2::new(center.x + r_outer * 0.05, center.y - r_outer * 0.35);
        let s_bot = Pos2::new(center.x - r_outer * 0.42, center.y + r_outer * 0.28);
        painter.line_segment([s_top, s_bot], Stroke::new(3.0, Color32::from_rgb(0, 195, 255)));
    }

    fn open_media_dialog(&mut self, layer: usize, col: usize) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("OSVJ — Cargar Video .MOV (DXV3) o Archivo Visual")
            .add_filter(
                "Videos & Visuales (.mov, .dxv, .mp4, .mkv, .avi, .gif, .png, .jpg)",
                &["mov", "dxv", "mp4", "mkv", "avi", "webm", "gif", "png", "jpg", "jpeg"],
            )
            .pick_file()
        {
            self.has_ffmpeg_cached = ClipBlock::resolve_ffmpeg_bin().is_some();
            let deck = self.engine.current_deck;
            let (iw, ih) = (self.engine.pool.width, self.engine.pool.height);
            let fps = self.engine.target_fps;
            let was_persistent = self.engine.decks[deck].grid[layer][col].persistent;

            self.engine.decks[deck].grid[layer][col].stop_stream_if_any();
            let mut new_block = ClipBlock::load_from_path(path, iw, ih, fps);
            new_block.persistent = was_persistent;

            self.engine.decks[deck].grid[layer][col] = new_block;
            if was_persistent {
                self.engine.set_clip_persistent(layer, col, true);
            }
            self.thumb_textures[layer][col] = None;
            self.thumb_uploaded_rev[layer][col] = 0;
            self.engine.preview_target = Some((layer, col));
        }
    }

    fn install_ffmpeg_one_click(&mut self) {
        if self.engine.ffmpeg_installing.load(Ordering::Relaxed) {
            return;
        }
        self.engine.ffmpeg_installing.store(true, Ordering::Relaxed);
        let flag = Arc::clone(&self.engine.ffmpeg_installing);

        thread::spawn(move || {
            let ps_script = r#"
                $ProgressPreference = 'SilentlyContinue';
                Invoke-WebRequest -Uri 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip' -OutFile 'ffmpeg_pkg.zip';
                Expand-Archive -Path 'ffmpeg_pkg.zip' -DestinationPath 'ffmpeg_tmp' -Force;
                $exe = Get-ChildItem -Path 'ffmpeg_tmp' -Filter 'ffmpeg.exe' -Recurse | Select-Object -First 1;
                if ($exe) { Copy-Item $exe.FullName -Destination '.\ffmpeg.exe' -Force; }
                Remove-Item 'ffmpeg_pkg.zip' -Force -ErrorAction SilentlyContinue;
                Remove-Item 'ffmpeg_tmp' -Recurse -Force -ErrorAction SilentlyContinue;
            "#;
            let _ = silent_command("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps_script])
                .status();
            flag.store(false, Ordering::Relaxed);
        });
    }

    fn start_mp4_to_mov_dxv3_conversion(&mut self) {
        if self.engine.converter.is_converting.load(Ordering::Relaxed) {
            return;
        }
        let Some(in_path) = self.engine.converter.input_path.clone() else {
            return;
        };
        let out_path = self.engine.converter.output_path.clone().unwrap_or_else(|| {
            let mut p = in_path.clone();
            let stem = in_path.file_stem().unwrap_or_default().to_string_lossy();
            p.set_file_name(format!("{}_DXV3.mov", stem));
            p
        });

        let Some(ffmpeg_bin) = ClipBlock::resolve_ffmpeg_bin() else {
            if let Ok(mut msg) = self.engine.converter.status_message.lock() {
                *msg = "⚠ Instala primero el motor desde el menú 'Herramientas DXV3 -> Descargar Motor'".into();
            }
            return;
        };

        self.engine.converter.is_converting.store(true, Ordering::Relaxed);
        if let Ok(mut msg) = self.engine.converter.status_message.lock() {
            *msg = format!(
                "⏳ Transcodificando hacia {}...",
                out_path.file_name().unwrap_or_default().to_string_lossy()
            );
        }

        let is_conv = Arc::clone(&self.engine.converter.is_converting);
        let status_arc = Arc::clone(&self.engine.converter.status_message);
        let last_out = Arc::clone(&self.engine.converter.last_converted_file);
        let profile = self.engine.converter.profile;
        let fps = self.engine.converter.target_fps;

        thread::spawn(move || {
            let pix_fmt = match profile {
                Dxv3ConvertProfile::NormalNoAlpha => "yuv420p",
                Dxv3ConvertProfile::HighWithAlpha => "yuva420p",
            };

            let status = silent_command(&ffmpeg_bin)
                .args([
                    "-y",
                    "-i",
                    &in_path.to_string_lossy(),
                    "-c:v",
                    "mjpeg",
                    "-q:v",
                    "2",
                    "-pix_fmt",
                    pix_fmt,
                    "-r",
                    &fps.to_string(),
                    "-an",
                    &out_path.to_string_lossy(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            if let Ok(s) = status {
                if s.success() {
                    if let Ok(mut msg) = status_arc.lock() {
                        *msg = format!("✔ Conversión DXV3 (.MOV) finalizada: {}", out_path.display());
                    }
                    if let Ok(mut lo) = last_out.lock() {
                        *lo = Some(out_path);
                    }
                } else if let Ok(mut msg) = status_arc.lock() {
                    *msg = "❌ Error al convertir el archivo a .MOV".into();
                }
            }
            is_conv.store(false, Ordering::Relaxed);
        });
    }

    fn resolve_slice_texture(&self, source: SliceSource) -> Option<TextureId> {
        match source {
            SliceSource::MasterComposition => self.master_tex.as_ref().map(|t| t.id()),
            SliceSource::Layer(idx) => self
                .layer_textures
                .get(idx)
                .and_then(|opt| opt.as_ref().map(|t| t.id())),
        }
    }

    fn paint_slice_on_rect(
        &self,
        painter: &egui::Painter,
        canvas_rect: Rect,
        slice: &OutputSlice,
        draw_wireframe: bool,
        is_selected: bool,
        normalize_centered: bool,
    ) {
        let Some(tex_id) = self.resolve_slice_texture(slice.source) else {
            return;
        };

        let (verts, tris) = slice.build_geometry();
        let bbox = slice.bounding_box();
        let mut mesh = egui::Mesh::with_texture(tex_id);

        let map_pt = |pos_norm: [f32; 2]| -> Pos2 {
            if normalize_centered {
                let [bx, by, bw, bh] = bbox;
                let nx = ((pos_norm[0] - bx) / bw) * 0.78 + 0.11;
                let ny = ((pos_norm[1] - by) / bh) * 0.78 + 0.11;
                Pos2::new(
                    canvas_rect.min.x + nx * canvas_rect.width(),
                    canvas_rect.min.y + ny * canvas_rect.height(),
                )
            } else {
                Pos2::new(
                    canvas_rect.min.x + pos_norm[0] * canvas_rect.width(),
                    canvas_rect.min.y + pos_norm[1] * canvas_rect.height(),
                )
            }
        };

        for (pos_norm, uv_norm) in &verts {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: map_pt(*pos_norm),
                uv: Pos2::new(uv_norm[0], uv_norm[1]),
                color: Color32::WHITE,
            });
        }

        for tri in &tris {
            mesh.add_triangle(tri[0] as u32, tri[1] as u32, tri[2] as u32);
        }

        painter.add(Shape::Mesh(mesh));

        if draw_wireframe {
            let s_col = slice.color_id();
            let stroke = Stroke::new(if is_selected { 1.8 } else { 1.0 }, s_col);

            if slice.mesh_mode_enabled && slice.mesh_points.len() >= 4 {
                let n = slice.mesh_subdivisions.max(2);
                if slice.mesh_points.len() == n * n {
                    for r in 0..n {
                        let row_pts: Vec<Pos2> = (0..n).map(|c| map_pt(slice.mesh_points[r * n + c])).collect();
                        painter.add(Shape::line(row_pts, stroke));
                    }
                    for c in 0..n {
                        let col_pts: Vec<Pos2> = (0..n).map(|r| map_pt(slice.mesh_points[r * n + c])).collect();
                        painter.add(Shape::line(col_pts, stroke));
                    }
                    if is_selected {
                        for pt in &slice.mesh_points {
                            let sp = map_pt(*pt);
                            painter.circle_filled(sp, 4.2, Color32::from_rgb(255, 215, 0));
                            painter.circle_stroke(sp, 4.2, Stroke::new(1.2, Color32::BLACK));
                        }
                    }
                }
            } else {
                let outline_pts: Vec<Pos2> = match slice.shape {
                    SliceShape::Circle | SliceShape::Hexagon => {
                        verts.iter().skip(1).map(|(p, _)| map_pt(*p)).collect()
                    }
                    _ => verts.iter().map(|(p, _)| map_pt(*p)).collect(),
                };
                painter.add(Shape::closed_line(outline_pts, stroke));

                if is_selected {
                    for cp in &slice.corner_points {
                        let sp = map_pt(*cp);
                        painter.circle_filled(sp, 5.0, Color32::WHITE);
                        painter.circle_stroke(sp, 5.0, Stroke::new(1.8, s_col));
                    }
                }
            }
        }
    }
}

impl eframe::App for VjApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let target_frame_dur = Duration::from_secs_f32(1.0 / (self.engine.target_fps.max(15) as f32));
        ctx.request_repaint_after(target_frame_dur);

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_immediate_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.show_exit_modal = true;
        }

        let elapsed = self.start_time.elapsed().as_secs_f32();

        if self.show_splash_screen && !self.splash_opened_manually && elapsed > 3.2 {
            self.show_splash_screen = false;
        }

        if self.last_autobackup_instant.elapsed() >= Duration::from_secs(300) {
            self.perform_backup_now(None);
        }

        // BLOQUEO DE RENDERIZADO POR RELOJ (Frame Limiter):
        // Si el usuario mueve el mouse a 240Hz, NO recalculamos ni subimos texturas hasta que se cumpla el intervalo de FPS
        let since_last_render = self.last_render_tick.elapsed();
        if since_last_render >= target_frame_dur || self.master_tex.is_none() {
            let dt = since_last_render.as_secs_f32().min(0.1);
            self.last_render_tick = Instant::now();

            if self.backup_indicator_timer > 0.0 {
                self.backup_indicator_timer = (self.backup_indicator_timer - dt).max(0.0);
            }

            self.engine.update_playheads(dt);

            if !self.has_ffmpeg_cached && !self.engine.ffmpeg_installing.load(Ordering::Relaxed) {
                if ClipBlock::resolve_ffmpeg_bin().is_some() {
                    self.has_ffmpeg_cached = true;
                }
            }

            self.engine.render_all_layers_and_master(elapsed);
            let w = self.engine.pool.width;
            let h = self.engine.pool.height;

            // Subir a VRAM únicamente las capas que tienen un clip activo
            for l_idx in 0..self.engine.layers.len() {
                if self.engine.layers[l_idx].playing_clip.is_some() && !self.engine.layers[l_idx].bypassed {
                    let l_img = ColorImage {
                        size: [w, h],
                        pixels: self.engine.pool.layer_buffers[l_idx].clone(),
                    };
                    let l_tex = self.layer_textures[l_idx].get_or_insert_with(|| {
                        ctx.load_texture(format!("gpu_layer_tex_{}", l_idx), l_img.clone(), TextureOptions::LINEAR)
                    });
                    l_tex.set(l_img, TextureOptions::LINEAR);
                }
            }

            let master_img = ColorImage {
                size: [w, h],
                pixels: self.engine.pool.master_buf.clone(),
            };
            let master_tex = self.master_tex.get_or_insert_with(|| {
                ctx.load_texture("gpu_master_out", master_img.clone(), TextureOptions::LINEAR)
            });
            master_tex.set(master_img, TextureOptions::LINEAR);

            // Preview Monitor ligero (320x180) para no desperdiciar RAM ni CPU
            if let Some((pl, pc)) = self.engine.preview_target {
                let deck = self.engine.current_deck;
                self.engine.decks[deck].grid[pl][pc].render_into(
                    &mut self.engine.pool.preview_buf,
                    PREVIEW_W,
                    PREVIEW_H,
                    elapsed,
                    false,
                );
                let prev_img = ColorImage {
                    size: [PREVIEW_W, PREVIEW_H],
                    pixels: self.engine.pool.preview_buf.clone(),
                };
                let prev_tex = self.preview_tex.get_or_insert_with(|| {
                    ctx.load_texture("gpu_preview_out", prev_img.clone(), TextureOptions::LINEAR)
                });
                prev_tex.set(prev_img, TextureOptions::LINEAR);
            }

            let active_tex_count = self.thumb_textures.iter().flatten().filter(|t| t.is_some()).count();
            self.telemetry.tick_render_frame(active_tex_count, (w, h));
        }

        // ================= BARRA DE MENÚ SUPERIOR =================
        egui::TopBottomPanel::top("osvj_top_menubar")
            .exact_height(28.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(12, 13, 17))
                    .inner_margin(egui::Margin::symmetric(8.0, 3.0)),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    let (logo_rect, logo_resp) = ui.allocate_exact_size(Vec2::new(22.0, 20.0), Sense::click());
                    Self::paint_osvj_vector_logo(&ui.painter_at(logo_rect), logo_rect.center(), 8.5, elapsed);
                    if logo_resp.clicked() {
                        self.show_splash_screen = true;
                        self.splash_opened_manually = true;
                    }

                    ui.label(RichText::new("OSVJ").strong().color(Color32::from_rgb(0, 240, 175)));
                    ui.label(RichText::new("OpenSourceVJ").size(10.5).color(Color32::GRAY));
                    ui.separator();

                    // 1. Menú Sesión
                    ui.menu_button("Sesión", |ui| {
                        if ui.button("💾 Guardar Sesión Actual").clicked() {
                            if let Some(p) = self.current_session_path.clone() {
                                self.perform_backup_now(Some(p));
                            } else if let Some(p) = rfd::FileDialog::new()
                                .set_file_name("show_osvj.osvj")
                                .add_filter("Sesión OSVJ (.osvj)", &["osvj", "json"])
                                .save_file()
                            {
                                self.current_session_path = Some(p.clone());
                                self.perform_backup_now(Some(p));
                            }
                            ui.close_menu();
                        }
                        if ui.button("💾 Guardar Sesión Como...").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .set_file_name("show_osvj.osvj")
                                .add_filter("Sesión OSVJ (.osvj)", &["osvj", "json"])
                                .save_file()
                            {
                                self.current_session_path = Some(p.clone());
                                self.perform_backup_now(Some(p));
                            }
                            ui.close_menu();
                        }
                        if ui.button("📂 Cargar Sesión...").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("Sesión OSVJ (.osvj, .json)", &["osvj", "json"])
                                .pick_file()
                            {
                                if self.engine.load_session_from_file(&p).is_ok() {
                                    self.current_session_path = Some(p);
                                    for r in &mut self.thumb_uploaded_rev {
                                        for c in r {
                                            *c = 0;
                                        }
                                    }
                                }
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("⏳ Hacer BackUp de Emergencia Ahora").clicked() {
                            self.perform_backup_now(None);
                            ui.close_menu();
                        }
                        if Path::new("osvj_autobackup.osvj").exists()
                            && ui.button("🔄 Restaurar Último Auto-BackUp").clicked()
                        {
                            let _ = self.engine.load_session_from_file(Path::new("osvj_autobackup.osvj"));
                            for r in &mut self.thumb_uploaded_rev {
                                for c in r {
                                    *c = 0;
                                }
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("🚪 Salir / Minimizar...").clicked() {
                            self.show_exit_modal = true;
                            ui.close_menu();
                        }
                    });

                    // 2. Menú Opciones
                    ui.menu_button("Opciones", |ui| {
                        ui.menu_button("🖥 Pantallas (Envío de Salida)", |ui| {
                            for m_idx in 0..self.engine.monitors.len() {
                                let lbl = self.engine.monitors[m_idx].display_label();
                                if ui.radio_value(&mut self.engine.selected_monitor_idx, m_idx, lbl).clicked() {
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui.button("🔄 Escanear Monitores Conectados").clicked() {
                                self.engine.monitors = WindowsMonitor::enumerate_windows_displays();
                            }
                            if ui.button("⚙ Configurar Resolución y FPS de Salida...").clicked() {
                                self.center_view = CenterWorkspaceView::DisplayRoutingView;
                                ui.close_menu();
                            }
                        });

                        ui.menu_button("📐 Resolución de Salida (Canvas)", |ui| {
                            let cur_res = self.engine.output_resolution;
                            let fps = self.engine.target_fps;
                            for (res, label) in [
                                ((1280, 720), "1280x720 (HD 720p)"),
                                ((1920, 1080), "1920x1080 (Full HD 1080p - Nativo)"),
                                ((2560, 1440), "2560x1440 (2K QHD)"),
                                ((3840, 2160), "3840x2160 (4K Ultra HD)"),
                            ] {
                                if ui.radio(cur_res == res, label).clicked() {
                                    self.engine.set_output_resolution_and_fps(res.0, res.1, fps);
                                    ui.close_menu();
                                }
                            }
                        });

                        ui.menu_button("⏱ Cuadros de Salida (FPS)", |ui| {
                            let (rw, rh) = self.engine.output_resolution;
                            let cur_fps = self.engine.target_fps;
                            for fps in [30u32, 50, 60, 120] {
                                if ui.radio(cur_fps == fps, format!("{} FPS", fps)).clicked() {
                                    self.engine.set_output_resolution_and_fps(rw, rh, fps);
                                    ui.close_menu();
                                }
                            }
                        });

                        ui.separator();
                        if ui.button("📐 Diseñador de Cortes / Slices").clicked() {
                            self.center_view = CenterWorkspaceView::SliceDesignerView;
                            ui.close_menu();
                        }
                        if ui.button("🔄 Conversor .MP4 a .MOV (DXV3)").clicked() {
                            self.center_view = CenterWorkspaceView::Dxv3ConverterView;
                            ui.close_menu();
                        }
                    });

                    // 3. Menú Deck
                    ui.menu_button("Deck", |ui| {
                        for d_idx in 0..self.engine.decks.len() {
                            let d_name = self.engine.decks[d_idx].name.clone();
                            if ui.radio_value(&mut self.engine.current_deck, d_idx, d_name).clicked() {
                                for r in &mut self.thumb_uploaded_rev {
                                    for c in r {
                                        *c = 0;
                                    }
                                }
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.button("➕ Añadir Nuevo Deck").clicked() {
                            let idx = self.engine.decks.len() + 1;
                            let cols = self.engine.num_columns;
                            let rows = self.engine.layers.len();
                            let mut new_grid = vec![vec![ClipBlock::empty(); cols]; rows];
                            let cur = self.engine.current_deck;
                            for l in 0..rows {
                                for c in 0..cols {
                                    if self.engine.decks[cur].grid[l][c].persistent {
                                        new_grid[l][c] = self.engine.decks[cur].grid[l][c].clone();
                                    }
                                }
                            }
                            self.engine.decks.push(crate::engine::Deck {
                                name: format!("Deck Show {}", idx),
                                layer_names: vec![
                                    format!("CAPA 3 · TOP (D{})", idx),
                                    format!("CAPA 2 · MID (D{})", idx),
                                    format!("CAPA 1 · BASE (D{})", idx),
                                ],
                                grid: new_grid,
                            });
                            self.engine.current_deck = self.engine.decks.len() - 1;
                            for r in &mut self.thumb_uploaded_rev {
                                for c in r {
                                    *c = 0;
                                }
                            }
                            ui.close_menu();
                        }
                        if ui
                            .button(RichText::new("🗑 Eliminar Deck Actual").color(Color32::from_rgb(255, 100, 100)))
                            .clicked()
                        {
                            let cur = self.engine.current_deck;
                            self.engine.delete_deck(cur);
                            for r in &mut self.thumb_uploaded_rev {
                                for c in r {
                                    *c = 0;
                                }
                            }
                            ui.close_menu();
                        }
                    });

                    // 4. Menú Herramientas DXV3
                    ui.menu_button("Herramientas DXV3", |ui| {
                        if ui.button("🔄 Conversor Integrado .MP4 -> .MOV (DXV3)").clicked() {
                            self.center_view = CenterWorkspaceView::Dxv3ConverterView;
                            ui.close_menu();
                        }
                        ui.separator();
                        if self.has_ffmpeg_cached {
                            ui.label(RichText::new("✔ Motor Decodificador DXV3 Activo").color(Color32::from_rgb(0, 235, 165)));
                        } else if ui.button("⬇ Descargar Motor DXV3/FFmpeg").clicked() {
                            self.install_ffmpeg_one_click();
                            ui.close_menu();
                        }
                    });

                    ui.separator();
                    ui.selectable_value(&mut self.center_view, CenterWorkspaceView::ClipMatrixGrid, "🎬 Matriz");
                    ui.selectable_value(&mut self.center_view, CenterWorkspaceView::SliceDesignerView, "📐 Cortes / Slices");
                    ui.selectable_value(&mut self.center_view, CenterWorkspaceView::Dxv3ConverterView, "🔄 Conversor DXV3");
                    ui.selectable_value(&mut self.center_view, CenterWorkspaceView::DisplayRoutingView, "🖥 Salida & Resolución");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.backup_indicator_timer > 0.0 {
                            egui::Frame::none()
                                .fill(Color32::from_rgb(255, 195, 0))
                                .rounding(Rounding::same(3.0))
                                .inner_margin(egui::Margin::symmetric(8.0, 2.0))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new("⏳ Haciendo BackUp")
                                            .size(11.5)
                                            .strong()
                                            .color(Color32::BLACK),
                                    );
                                });
                            ui.add_space(6.0);
                        }

                        let (rw, rh) = self.engine.output_resolution;
                        ui.label(
                            RichText::new(format!("Canvas: {}x{} @ {}fps", rw, rh, self.engine.target_fps))
                                .size(11.0)
                                .strong()
                                .color(Color32::from_rgb(0, 235, 170)),
                        );
                    });
                });
            });

        // ================= BARRA DE ESTADO INFERIOR =================
        egui::TopBottomPanel::bottom("osvj_bottom_statusbar")
            .exact_height(22.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(10, 11, 14))
                    .inner_margin(egui::Margin::symmetric(8.0, 2.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.backup_indicator_timer > 0.0 {
                        ui.label(
                            RichText::new("⏳ Haciendo BackUp...")
                                .size(11.0)
                                .strong()
                                .color(Color32::from_rgb(255, 210, 40)),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "FPS {:.1} / {}",
                                self.telemetry.fps, self.engine.target_fps
                            ))
                            .size(11.0)
                            .strong()
                            .color(Color32::from_rgb(0, 235, 165)),
                        );
                        ui.separator();
                        ui.label(RichText::new(format!("VRAM {:.0} MB", self.telemetry.vram_est_mb)).size(11.0).color(Color32::from_rgb(255, 175, 50)));
                        ui.separator();
                        ui.label(
                            RichText::new(format!("RAM {:.0}% ({:.0} MB)", self.telemetry.total_ram_pct, self.telemetry.ram_used_mb))
                                .size(11.0)
                                .color(Color32::from_rgb(255, 115, 90)),
                        );
                        ui.separator();
                        ui.label(RichText::new(format!("CPU {:.1}%", self.telemetry.cpu_usage)).size(11.0).color(Color32::LIGHT_GRAY));
                    });
                });
            });

        // ================= PANEL IZQUIERDO REDIMENSIONABLE: 3 MONITORES =================
        egui::SidePanel::left("osvj_resizable_monitors_dock")
            .resizable(true)
            .default_width(420.0)
            .width_range(320.0..=780.0)
            .frame(egui::Frame::none().fill(Color32::from_rgb(14, 15, 19)).inner_margin(egui::Margin::same(6.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("📺 MONITORES DE SALIDA").size(11.0).strong().color(Color32::from_rgb(0, 235, 175)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Slider::new(&mut self.monitor_height_scale, 0.75..=1.65).show_value(false));
                        ui.label(RichText::new("Zoom:").size(10.5).color(Color32::GRAY));
                    });
                });
                ui.separator();

                let cur_layer_names = self.engine.current_layer_names();

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let avail_w = (ui.available_width() - 10.0).max(280.0);
                    let base_h = (avail_w * (9.0 / 16.0) * self.monitor_height_scale).clamp(135.0, 420.0);

                    // 1. SCREEN MONITOR
                    if self.maximized_monitor.is_none() || self.maximized_monitor == Some(0) {
                        let h_mon = if self.maximized_monitor == Some(0) { base_h * 1.65 } else { base_h };
                        egui::Frame::none()
                            .fill(Color32::from_rgb(20, 22, 27))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(36, 40, 50)))
                            .rounding(Rounding::same(4.0))
                            .inner_margin(egui::Margin::same(5.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("1. SCREEN MONITOR · LIVEVIEW").size(10.5).strong().color(Color32::from_rgb(0, 235, 170)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let max_lbl = if self.maximized_monitor == Some(0) { "🗗 Normal" } else { "🔍 Agrandar" };
                                        if ui.small_button(max_lbl).clicked() {
                                            self.maximized_monitor = if self.maximized_monitor == Some(0) { None } else { Some(0) };
                                        }
                                        ui.checkbox(&mut self.show_slice_wireframes_on_liveview, RichText::new("Guías").size(10.5));
                                    });
                                });
                                let (rect_live, _) = ui.allocate_exact_size(Vec2::new(avail_w, h_mon), Sense::hover());
                                let live_painter = ui.painter_at(rect_live);
                                live_painter.rect_filled(rect_live, 2.0, Color32::BLACK);

                                for (idx, slice) in self.engine.slices.iter().enumerate() {
                                    if !slice.send_to_screen {
                                        continue;
                                    }
                                    self.paint_slice_on_rect(
                                        &live_painter,
                                        rect_live,
                                        slice,
                                        self.show_slice_wireframes_on_liveview,
                                        idx == self.engine.selected_slice,
                                        false,
                                    );
                                }
                            });
                        ui.add_space(5.0);
                    }

                    // 2. PREVIEW MONITOR
                    if self.maximized_monitor.is_none() || self.maximized_monitor == Some(1) {
                        let h_mon = if self.maximized_monitor == Some(1) { base_h * 1.65 } else { base_h };
                        egui::Frame::none()
                            .fill(Color32::from_rgb(20, 22, 27))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(36, 40, 50)))
                            .rounding(Rounding::same(4.0))
                            .inner_margin(egui::Margin::same(5.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("2. PREVIEW MONITOR · CLIP").size(10.5).strong().color(Color32::from_rgb(255, 205, 50)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let max_lbl = if self.maximized_monitor == Some(1) { "🗗 Normal" } else { "🔍 Agrandar" };
                                        if ui.small_button(max_lbl).clicked() {
                                            self.maximized_monitor = if self.maximized_monitor == Some(1) { None } else { Some(1) };
                                        }
                                        if let Some((pl, pc)) = self.engine.preview_target {
                                            let bname = &self.engine.decks[self.engine.current_deck].grid[pl][pc].name;
                                            let short: String = bname.chars().take(20).collect();
                                            ui.label(RichText::new(short).size(10.5).color(Color32::GRAY));
                                        }
                                    });
                                });
                                let (rect_prev, _) = ui.allocate_exact_size(Vec2::new(avail_w, h_mon), Sense::hover());
                                let prev_painter = ui.painter_at(rect_prev);
                                prev_painter.rect_filled(rect_prev, 2.0, Color32::from_rgb(8, 8, 10));
                                if let Some(tex) = &self.preview_tex {
                                    prev_painter.image(
                                        tex.id(),
                                        rect_prev,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                }
                            });
                        ui.add_space(5.0);
                    }

                    // 3. SLICE PREVIEW
                    if self.maximized_monitor.is_none() || self.maximized_monitor == Some(2) {
                        let h_mon = if self.maximized_monitor == Some(2) { base_h * 1.65 } else { base_h };
                        egui::Frame::none()
                            .fill(Color32::from_rgb(20, 22, 27))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(36, 40, 50)))
                            .rounding(Rounding::same(4.0))
                            .inner_margin(egui::Margin::same(5.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("3. SLICE EN VIVO · PREVIEW SEPARADO").size(10.5).strong().color(Color32::from_rgb(0, 195, 255)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let max_lbl = if self.maximized_monitor == Some(2) { "🗗 Normal" } else { "🔍 Agrandar" };
                                        if ui.small_button(max_lbl).clicked() {
                                            self.maximized_monitor = if self.maximized_monitor == Some(2) { None } else { Some(2) };
                                        }
                                        egui::ComboBox::from_id_source("quick_slice_selector")
                                            .width(125.0)
                                            .selected_text(
                                                self.engine
                                                    .slices
                                                    .get(self.engine.selected_slice)
                                                    .map(|s| s.name.as_str())
                                                    .unwrap_or("Slice"),
                                            )
                                            .show_ui(ui, |ui| {
                                                for i in 0..self.engine.slices.len() {
                                                    let sname = self.engine.slices[i].name.clone();
                                                    ui.selectable_value(&mut self.engine.selected_slice, i, sname);
                                                }
                                            });
                                    });
                                });
                                let (rect_slice_prev, _) = ui.allocate_exact_size(Vec2::new(avail_w, h_mon), Sense::hover());
                                let slice_painter = ui.painter_at(rect_slice_prev);
                                slice_painter.rect_filled(rect_slice_prev, 2.0, Color32::from_rgb(9, 10, 13));

                                if let Some(slice) = self.engine.slices.get(self.engine.selected_slice) {
                                    self.paint_slice_on_rect(&slice_painter, rect_slice_prev, slice, true, true, true);
                                    let src_lbl = slice.source.label(&cur_layer_names);
                                    slice_painter.text(
                                        rect_slice_prev.min + Vec2::new(6.0, 4.0),
                                        Align2::LEFT_TOP,
                                        format!("{} [{}]", slice.name, src_lbl),
                                        FontId::proportional(10.5),
                                        slice.color_id(),
                                    );
                                }
                            });
                    }
                });
            });

        // ================= DOCK INFERIOR =================
        egui::TopBottomPanel::bottom("osvj_bottom_docks")
            .resizable(true)
            .default_height(225.0)
            .height_range(195.0..=340.0)
            .frame(egui::Frame::none().fill(Color32::from_rgb(15, 16, 20)).inner_margin(egui::Margin::same(6.0)))
            .show(ctx, |ui| {
                ui.columns(3, |cols| {
                    // COLUMNA 1: PROPIEDADES DE CAPA
                    egui::Frame::none()
                        .fill(Color32::from_rgb(20, 22, 27))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(35, 39, 48)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(6.0))
                        .show(&mut cols[0], |ui| {
                            let cur_deck = self.engine.current_deck;
                            let sel_l = self.selected_layer;
                            let layer_title = self.engine.decks[cur_deck].layer_names[sel_l].clone();

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("🎛 CAPA (DECK {}) · {}", cur_deck + 1, layer_title))
                                        .size(11.0)
                                        .strong()
                                        .color(Color32::from_rgb(0, 235, 175)),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Reset FX").clicked() {
                                        self.engine.layers[sel_l].fx = Default::default();
                                        self.engine.layers[sel_l].transform = Default::default();
                                    }
                                });
                            });
                            ui.separator();

                            egui::Grid::new("osvj_layer_inspector").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                                ui.label(RichText::new("Nombre en este Deck:").size(11.0));
                                ui.text_edit_singleline(&mut self.engine.decks[cur_deck].layer_names[sel_l]);
                                ui.end_row();

                                let layer = &mut self.engine.layers[sel_l];
                                ui.label(RichText::new("Escala & Pos X/Y:").size(11.0));
                                ui.horizontal(|ui| {
                                    ui.add(egui::DragValue::new(&mut layer.transform.scale).speed(1.0).clamp_range(20.0..=300.0).suffix("%"));
                                    ui.add(egui::DragValue::new(&mut layer.transform.pos_x).speed(1.0).prefix("X:"));
                                    ui.add(egui::DragValue::new(&mut layer.transform.pos_y).speed(1.0).prefix("Y:"));
                                });
                                ui.end_row();

                                ui.label(RichText::new("FX RGB Shift:").size(11.0));
                                ui.add(egui::Slider::new(&mut layer.fx.rgb_shift, 0.0..=1.0));
                                ui.end_row();

                                ui.label(RichText::new("FX Strobe:").size(11.0));
                                ui.add(egui::Slider::new(&mut layer.fx.strobe, 0.0..=1.0));
                                ui.end_row();

                                ui.label(RichText::new("Filtros GPU:").size(11.0));
                                ui.horizontal(|ui| {
                                    ui.toggle_value(&mut layer.fx.mirror_x, "Mirror X");
                                    ui.toggle_value(&mut layer.fx.mirror_y, "Mirror Y");
                                    ui.toggle_value(&mut layer.fx.invert, "Invertir");
                                });
                                ui.end_row();
                            });
                        });

                    // COLUMNA 2: CLIP SELECCIONADO & PERSISTENCIA
                    egui::Frame::none()
                        .fill(Color32::from_rgb(20, 22, 27))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(35, 39, 48)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(6.0))
                        .show(&mut cols[1], |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("🎬 CLIP · TRANSPORTE & PERSISTENCIA").size(11.0).strong().color(Color32::from_rgb(255, 205, 50)));
                            });
                            ui.separator();

                            if let Some((pl, pc)) = self.engine.preview_target {
                                let deck = self.engine.current_deck;
                                let is_live_here = self.engine.layers[pl].playing_origin == Some((deck, pc));

                                let (b_name, b_codec, mut b_speed, mut b_paused, mut b_playhead, mut b_persist, status_txt, non_mov_path) = {
                                    let b = &self.engine.decks[deck].grid[pl][pc];
                                    let mut non_mov = None;
                                    let st = match &b.source {
                                        ClipSource::MovDxv3Stream(s) => {
                                            let ext = s.path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
                                            if ext != "mov" && ext != "dxv" && ext != "gif" {
                                                non_mov = Some(s.path.clone());
                                            }
                                            if let Ok(lock) = s.status.try_lock() {
                                                match &*lock {
                                                    DecodeStatus::StandbyThumbReady => {
                                                        "✔ Miniatura en VRAM (0% CPU en reposo)".to_string()
                                                    }
                                                    DecodeStatus::StreamingActive(rw, rh) => {
                                                        format!("● Stream Activo en Vivo ({}x{} -> GPU)", rw, rh)
                                                    }
                                                    DecodeStatus::MissingFfmpeg => "⚠ Descarga el Motor DXV3 desde el menú superior".to_string(),
                                                    DecodeStatus::DecodeError(e) => format!("❌ {}", e),
                                                }
                                            } else {
                                                "Listo".to_string()
                                            }
                                        }
                                        ClipSource::StaticImage(_) => "✔ Textura en VRAM lista".to_string(),
                                        ClipSource::Generator(_) => "⚡ Generador Ligero GPU".to_string(),
                                        ClipSource::Empty => "Bloque vacío (Haz clic en 📂 para cargar)".to_string(),
                                    };
                                    (
                                        b.name.clone(),
                                        b.codec_tag.clone(),
                                        b.speed,
                                        b.paused,
                                        b.playhead,
                                        b.persistent,
                                        st,
                                        non_mov,
                                    )
                                };

                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(if b_name.is_empty() { "Slot Vacío" } else { &b_name }).strong().size(12.5));
                                    if is_live_here {
                                        ui.label(RichText::new("● EN VIVO").size(10.5).color(Color32::from_rgb(0, 250, 145)).strong());
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui
                                            .checkbox(&mut b_persist, RichText::new("📌 Persistente").size(10.5).color(Color32::from_rgb(0, 230, 255)))
                                            .changed()
                                        {
                                            self.engine.set_clip_persistent(pl, pc, b_persist);
                                        }
                                    });
                                });
                                ui.label(RichText::new(status_txt).size(11.0).color(Color32::from_rgb(0, 220, 165)));
                                ui.label(RichText::new(format!("Formato: {}", b_codec)).size(10.5).color(Color32::GRAY));

                                ui.horizontal(|ui| {
                                    if ui.button(if b_paused { "▶ Play" } else { "⏸ Pausa" }).clicked() {
                                        b_paused = !b_paused;
                                    }
                                    ui.add(egui::Slider::new(&mut b_playhead, 0.0..=1.0).show_value(false));
                                    ui.label("Vel:");
                                    ui.add(egui::DragValue::new(&mut b_speed).speed(0.05).clamp_range(0.1..=4.0).suffix("x"));
                                });

                                {
                                    let b = &mut self.engine.decks[deck].grid[pl][pc];
                                    b.speed = b_speed;
                                    b.paused = b_paused;
                                    b.playhead = b_playhead;
                                    if is_live_here {
                                        if let Some(live_b) = &mut self.engine.layers[pl].playing_clip {
                                            live_b.speed = b_speed;
                                            live_b.paused = b_paused;
                                            live_b.playhead = b_playhead;
                                        }
                                    }
                                }

                                ui.horizontal(|ui| {
                                    if ui.button("📂 Cargar Video").clicked() {
                                        self.open_media_dialog(pl, pc);
                                    }
                                    if ui.button(RichText::new("▶ Disparar a Capa").color(Color32::from_rgb(0, 245, 155))).clicked() {
                                        self.engine.trigger_clip(pl, pc);
                                    }
                                    if let Some(mp4_p) = non_mov_path {
                                        if ui.button(RichText::new("🔄 Convertir a .MOV").color(Color32::from_rgb(255, 200, 50))).clicked() {
                                            self.engine.converter.input_path = Some(mp4_p.clone());
                                            let mut out_p = mp4_p.clone();
                                            let stem = mp4_p.file_stem().unwrap_or_default().to_string_lossy();
                                            out_p.set_file_name(format!("{}_DXV3.mov", stem));
                                            self.engine.converter.output_path = Some(out_p);
                                            self.center_view = CenterWorkspaceView::Dxv3ConverterView;
                                        }
                                    }
                                });
                            }
                        });

                    // COLUMNA 3: RELOJ DEL SISTEMA Y TELEMETRÍA
                    egui::Frame::none()
                        .fill(Color32::from_rgb(20, 22, 27))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(35, 39, 48)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(6.0))
                        .show(&mut cols[2], |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("⏱ SYSTEM CLOCK & RECURSOS").size(11.0).strong().color(Color32::from_rgb(0, 215, 255)));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if self.backup_indicator_timer > 0.0 {
                                        ui.label(RichText::new("⏳ Haciendo BackUp").size(10.5).strong().color(Color32::YELLOW));
                                    } else {
                                        ui.label(RichText::new(format!("{:.1} FPS", self.telemetry.fps)).size(11.0).strong().color(Color32::from_rgb(0, 245, 160)));
                                    }
                                });
                            });
                            ui.separator();

                            let now = chrono::Local::now();
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("System Time").size(10.0).color(Color32::GRAY));
                                    ui.label(
                                        RichText::new(now.format("%H:%M:%S").to_string())
                                            .size(21.0)
                                            .monospace()
                                            .strong()
                                            .color(Color32::WHITE),
                                    );
                                });
                                ui.add_space(16.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("Layer Live Time").size(10.0).color(Color32::GRAY));
                                    let ph = self.engine.layers[self.selected_layer]
                                        .playing_clip
                                        .as_ref()
                                        .map(|c| c.playhead * c.duration_secs)
                                        .unwrap_or(0.0);
                                    let mins = (ph as u32) / 60;
                                    let secs = (ph as u32) % 60;
                                    let centis = ((ph.fract()) * 100.0) as u32;
                                    ui.label(
                                        RichText::new(format!("{:02}:{:02}.{:02}", mins, secs, centis))
                                            .size(21.0)
                                            .monospace()
                                            .strong()
                                            .color(Color32::from_rgb(0, 235, 170)),
                                    );
                                });
                            });

                            ui.add_space(4.0);
                            egui::Grid::new("telemetry_bars_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                                ui.label(RichText::new("CPU Uso:").size(10.5).color(Color32::LIGHT_GRAY));
                                ui.add(
                                    egui::ProgressBar::new((self.telemetry.cpu_usage / 100.0).clamp(0.0, 1.0))
                                        .desired_height(13.0)
                                        .text(RichText::new(format!("{:.1}%", self.telemetry.cpu_usage)).size(10.0)),
                                );
                                ui.end_row();

                                ui.label(RichText::new("RAM App:").size(10.5).color(Color32::LIGHT_GRAY));
                                ui.add(
                                    egui::ProgressBar::new((self.telemetry.total_ram_pct / 100.0).clamp(0.0, 1.0))
                                        .desired_height(13.0)
                                        .text(
                                            RichText::new(format!(
                                                "{:.0} MB ({:.0}% Sys)",
                                                self.telemetry.ram_used_mb, self.telemetry.total_ram_pct
                                            ))
                                            .size(10.0),
                                        ),
                                );
                                ui.end_row();

                                ui.label(RichText::new("VRAM GPU:").size(10.5).color(Color32::LIGHT_GRAY));
                                ui.add(
                                    egui::ProgressBar::new((self.telemetry.vram_est_mb / 1024.0).clamp(0.05, 1.0))
                                        .desired_height(13.0)
                                        .text(RichText::new(format!("{:.0} MB WGPU", self.telemetry.vram_est_mb)).size(10.0)),
                                );
                                ui.end_row();
                            });
                        });
                });
            });

        // ================= ÁREA CENTRAL =================
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(12, 13, 16)).inner_margin(egui::Margin::same(6.0)))
            .show(ctx, |ui| {
                match self.center_view {
                    // ---------------- VISTA 1: MATRIZ PRINCIPAL ----------------
                    CenterWorkspaceView::ClipMatrixGrid => {
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                            egui::Grid::new("osvj_matrix_grid").spacing([5.0, 6.0]).show(ui, |ui| {
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(22, 24, 30))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(38, 42, 52)))
                                    .rounding(Rounding::same(3.0))
                                    .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                                    .show(ui, |ui| {
                                        ui.set_min_size(Vec2::new(210.0, 26.0));
                                        ui.horizontal(|ui| {
                                            if ui
                                                .small_button(RichText::new("✖").color(Color32::from_rgb(255, 90, 90)))
                                                .clicked()
                                            {
                                                for l in &mut self.engine.layers {
                                                    if let Some(old) = &l.playing_clip {
                                                        old.stop_stream_if_any();
                                                    }
                                                    l.playing_clip = None;
                                                    l.playing_origin = None;
                                                }
                                            }
                                            ui.label(RichText::new("Master").size(11.0).strong());
                                            ui.add(egui::Slider::new(&mut self.engine.master_opacity, 0.0..=1.0).show_value(false));
                                        });
                                    });

                                let mut trigger_col_req = None;
                                for c in 0..self.engine.num_columns {
                                    let col_active = self
                                        .engine
                                        .layers
                                        .iter()
                                        .any(|l| l.playing_origin == Some((self.engine.current_deck, c)));

                                    let btn = egui::Button::new(
                                        RichText::new(format!("▶ Columna {}", c + 1))
                                            .size(11.0)
                                            .strong()
                                            .color(if col_active {
                                                Color32::from_rgb(0, 245, 165)
                                            } else {
                                                Color32::from_rgb(190, 195, 205)
                                            }),
                                    )
                                    .min_size(Vec2::new(134.0, 26.0));

                                    if ui.add(btn).clicked() {
                                        trigger_col_req = Some(c);
                                    }
                                }
                                if let Some(c) = trigger_col_req {
                                    self.engine.trigger_column(c);
                                }
                                ui.end_row();

                                let num_layers = self.engine.layers.len();
                                for l_idx in 0..num_layers {
                                    let is_layer_sel = self.selected_layer == l_idx;
                                    let cur_deck = self.engine.current_deck;
                                    let deck_layer_name = self.engine.decks[cur_deck].layer_names[l_idx].clone();

                                    egui::Frame::none()
                                        .fill(if is_layer_sel {
                                            Color32::from_rgb(24, 34, 40)
                                        } else {
                                            Color32::from_rgb(19, 21, 26)
                                        })
                                        .stroke(Stroke::new(
                                            1.2,
                                            if is_layer_sel {
                                                Color32::from_rgb(0, 230, 170)
                                            } else {
                                                Color32::from_rgb(38, 42, 50)
                                            },
                                        ))
                                        .rounding(Rounding::same(3.0))
                                        .inner_margin(egui::Margin::same(6.0))
                                        .show(ui, |ui| {
                                            ui.set_min_size(Vec2::new(210.0, 98.0));
                                            let layer = &mut self.engine.layers[l_idx];

                                            egui::Frame::none()
                                                .fill(Color32::from_rgb(0, 130, 98))
                                                .rounding(Rounding::same(2.0))
                                                .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        if ui
                                                            .selectable_label(
                                                                is_layer_sel,
                                                                RichText::new(&deck_layer_name).size(12.5).strong().color(Color32::WHITE),
                                                            )
                                                            .clicked()
                                                        {
                                                            self.selected_layer = l_idx;
                                                        }
                                                    });
                                                });

                                            ui.add_space(3.0);
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .small_button(RichText::new("✖").color(Color32::from_rgb(255, 90, 90)))
                                                    .clicked()
                                                {
                                                    if let Some(old) = &layer.playing_clip {
                                                        old.stop_stream_if_any();
                                                    }
                                                    layer.playing_clip = None;
                                                    layer.playing_origin = None;
                                                }
                                                ui.toggle_value(&mut layer.bypassed, RichText::new("B").size(11.0).strong());
                                                ui.toggle_value(&mut layer.solo, RichText::new("S").size(11.0).strong());

                                                ui.selectable_value(&mut layer.bus, BusAssign::CrossA, RichText::new("A").size(10.5));
                                                ui.selectable_value(&mut layer.bus, BusAssign::Through, RichText::new("II").size(10.5));
                                                ui.selectable_value(&mut layer.bus, BusAssign::CrossB, RichText::new("B").size(10.5));
                                            });

                                            ui.horizontal(|ui| {
                                                ui.add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).show_value(false));
                                                if let Some((orig_d, _)) = layer.playing_origin {
                                                    if orig_d != cur_deck {
                                                        ui.label(
                                                            RichText::new(format!("D{}", orig_d + 1))
                                                                .size(10.0)
                                                                .color(Color32::from_rgb(0, 240, 165))
                                                                .strong(),
                                                        );
                                                    }
                                                }
                                            });
                                        });

                                    for c_idx in 0..self.engine.num_columns {
                                        let deck = self.engine.current_deck;
                                        let is_live_this_cell = self.engine.layers[l_idx].playing_origin == Some((deck, c_idx));
                                        let is_preview = self.engine.preview_target == Some((l_idx, c_idx));
                                        let is_persistent = self.engine.decks[deck].grid[l_idx][c_idx].persistent;

                                        let border_stroke = if is_live_this_cell {
                                            Stroke::new(2.0, Color32::from_rgb(0, 245, 150))
                                        } else if is_preview {
                                            Stroke::new(1.5, Color32::from_rgb(255, 205, 45))
                                        } else if is_persistent {
                                            Stroke::new(1.3, Color32::from_rgb(0, 195, 255))
                                        } else {
                                            Stroke::new(1.0, Color32::from_rgb(34, 38, 46))
                                        };

                                        egui::Frame::none()
                                            .fill(Color32::from_rgb(18, 20, 25))
                                            .stroke(border_stroke)
                                            .rounding(Rounding::same(3.0))
                                            .show(ui, |ui| {
                                                ui.set_min_size(Vec2::new(134.0, 98.0));
                                                ui.vertical(|ui| {
                                                    let (thumb_rect, resp) = ui.allocate_exact_size(Vec2::new(134.0, 72.0), Sense::click());
                                                    ui.painter().rect_filled(thumb_rect, 2.0, Color32::from_rgb(10, 11, 14));

                                                    let is_empty = self.engine.decks[deck].grid[l_idx][c_idx].is_empty();
                                                    let playhead = self.engine.decks[deck].grid[l_idx][c_idx].playhead;
                                                    let cur_rev = self.engine.decks[deck].grid[l_idx][c_idx].current_thumb_rev();

                                                    if !is_empty {
                                                        if is_live_this_cell {
                                                            if let Some(Some(layer_tex)) = self.layer_textures.get(l_idx) {
                                                                ui.painter().image(
                                                                    layer_tex.id(),
                                                                    thumb_rect,
                                                                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                                                    Color32::WHITE,
                                                                );
                                                            }
                                                        } else {
                                                            if self.thumb_textures[l_idx][c_idx].is_none()
                                                                || self.thumb_uploaded_rev[l_idx][c_idx] != cur_rev
                                                            {
                                                                self.engine.decks[deck].grid[l_idx][c_idx].render_thumb_into(
                                                                    &mut self.thumb_scratch,
                                                                    THUMB_W,
                                                                    THUMB_H,
                                                                );
                                                                let t_img = ColorImage {
                                                                    size: [THUMB_W, THUMB_H],
                                                                    pixels: self.thumb_scratch.clone(),
                                                                };
                                                                let tex = self.thumb_textures[l_idx][c_idx].get_or_insert_with(|| {
                                                                    ctx.load_texture(
                                                                        format!("clip_thumb_{}_{}", l_idx, c_idx),
                                                                        t_img.clone(),
                                                                        TextureOptions::LINEAR,
                                                                    )
                                                                });
                                                                tex.set(t_img, TextureOptions::LINEAR);
                                                                self.thumb_uploaded_rev[l_idx][c_idx] = cur_rev;
                                                            }

                                                            if let Some(tex) = &self.thumb_textures[l_idx][c_idx] {
                                                                ui.painter().image(
                                                                    tex.id(),
                                                                    thumb_rect,
                                                                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                                                    Color32::WHITE,
                                                                );
                                                            }
                                                        }

                                                        let prog_rect = Rect::from_min_size(
                                                            Pos2::new(thumb_rect.min.x, thumb_rect.max.y - 3.0),
                                                            Vec2::new(thumb_rect.width() * playhead, 3.0),
                                                        );
                                                        ui.painter().rect_filled(prog_rect, 0.0, Color32::from_rgb(0, 235, 165));
                                                    } else {
                                                        ui.painter().text(
                                                            thumb_rect.center(),
                                                            Align2::CENTER_CENTER,
                                                            "+ Cargar Clip",
                                                            FontId::proportional(10.5),
                                                            Color32::from_rgb(70, 75, 88),
                                                        );
                                                    }

                                                    if is_live_this_cell {
                                                        ui.painter().rect_filled(
                                                            Rect::from_min_size(thumb_rect.min + Vec2::new(4.0, 4.0), Vec2::new(36.0, 14.0)),
                                                            2.0,
                                                            Color32::from_rgb(0, 230, 135),
                                                        );
                                                        ui.painter().text(
                                                            thumb_rect.min + Vec2::new(22.0, 11.0),
                                                            Align2::CENTER_CENTER,
                                                            "LIVE",
                                                            FontId::proportional(9.5),
                                                            Color32::BLACK,
                                                        );
                                                    }

                                                    if resp.clicked() {
                                                        if is_empty {
                                                            self.open_media_dialog(l_idx, c_idx);
                                                        } else {
                                                            self.engine.preview_target = Some((l_idx, c_idx));
                                                            self.selected_layer = l_idx;
                                                        }
                                                    }
                                                    if resp.double_clicked() && !is_empty {
                                                        self.engine.trigger_clip(l_idx, c_idx);
                                                    }
                                                    if resp.secondary_clicked() {
                                                        if !is_empty {
                                                            self.engine.trigger_clip(l_idx, c_idx);
                                                            self.engine.preview_target = Some((l_idx, c_idx));
                                                        } else {
                                                            self.open_media_dialog(l_idx, c_idx);
                                                        }
                                                    }

                                                    ui.horizontal(|ui| {
                                                        ui.add_space(2.0);
                                                        if !is_empty {
                                                            if ui
                                                                .small_button(RichText::new("▶").size(9.5).color(if is_live_this_cell {
                                                                    Color32::from_rgb(0, 245, 150)
                                                                } else {
                                                                    Color32::LIGHT_GRAY
                                                                }))
                                                                .clicked()
                                                            {
                                                                self.engine.trigger_clip(l_idx, c_idx);
                                                                self.engine.preview_target = Some((l_idx, c_idx));
                                                            }
                                                            if ui
                                                                .small_button(RichText::new("📌").size(9.5).color(if is_persistent {
                                                                    Color32::from_rgb(0, 220, 255)
                                                                } else {
                                                                    Color32::DARK_GRAY
                                                                }))
                                                                .on_hover_text("Fijar clip como Persistente en todos los Decks")
                                                                .clicked()
                                                            {
                                                                self.engine.set_clip_persistent(l_idx, c_idx, !is_persistent);
                                                            }
                                                        }
                                                        if ui.small_button(RichText::new("📂").size(9.5)).clicked() {
                                                            self.open_media_dialog(l_idx, c_idx);
                                                        }

                                                        let b_name = self.engine.decks[deck].grid[l_idx][c_idx].name.clone();
                                                        let label_txt: String = b_name.chars().take(10).collect();
                                                        ui.label(
                                                            RichText::new(label_txt)
                                                                .size(10.0)
                                                                .color(if is_live_this_cell {
                                                                    Color32::WHITE
                                                                } else {
                                                                    Color32::from_rgb(165, 170, 180)
                                                                }),
                                                        );
                                                    });
                                                });
                                            });
                                    }
                                    ui.end_row();
                                }
                            });
                        });

                        ui.add_space(4.0);
                        // BARRA DE DECKS + BPM TAP
                        egui::Frame::none()
                            .fill(Color32::from_rgb(20, 22, 27))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(36, 40, 50)))
                            .rounding(Rounding::same(3.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("DECKS").size(10.5).strong().color(Color32::GRAY));
                                    for d_idx in 0..self.engine.decks.len() {
                                        let is_cur = self.engine.current_deck == d_idx;
                                        let has_playing = self
                                            .engine
                                            .layers
                                            .iter()
                                            .any(|l| l.playing_origin.map(|(d, _)| d == d_idx).unwrap_or(false));

                                        let tab_title = if has_playing {
                                            format!(" ● {} ", self.engine.decks[d_idx].name)
                                        } else {
                                            format!(" {} ", self.engine.decks[d_idx].name)
                                        };

                                        if ui
                                            .selectable_label(
                                                is_cur,
                                                RichText::new(tab_title).size(11.5).strong().color(if is_cur {
                                                    Color32::BLACK
                                                } else if has_playing {
                                                    Color32::from_rgb(0, 235, 165)
                                                } else {
                                                    Color32::LIGHT_GRAY
                                                }),
                                            )
                                            .clicked()
                                        {
                                            self.engine.current_deck = d_idx;
                                            for r in &mut self.thumb_uploaded_rev {
                                                for c in r {
                                                    *c = 0;
                                                }
                                            }
                                        }
                                    }

                                    if ui.small_button("➕").on_hover_text("Añadir Deck").clicked() {
                                        let idx = self.engine.decks.len() + 1;
                                        let cols = self.engine.num_columns;
                                        let rows = self.engine.layers.len();
                                        let mut new_grid = vec![vec![ClipBlock::empty(); cols]; rows];
                                        let cur = self.engine.current_deck;
                                        for l in 0..rows {
                                            for c in 0..cols {
                                                if self.engine.decks[cur].grid[l][c].persistent {
                                                    new_grid[l][c] = self.engine.decks[cur].grid[l][c].clone();
                                                }
                                            }
                                        }
                                        self.engine.decks.push(crate::engine::Deck {
                                            name: format!("Deck Show {}", idx),
                                            layer_names: vec![
                                                format!("CAPA 3 · TOP (D{})", idx),
                                                format!("CAPA 2 · MID (D{})", idx),
                                                format!("CAPA 1 · BASE (D{})", idx),
                                            ],
                                            grid: new_grid,
                                        });
                                        self.engine.current_deck = self.engine.decks.len() - 1;
                                        for r in &mut self.thumb_uploaded_rev {
                                            for c in r {
                                                *c = 0;
                                            }
                                        }
                                    }

                                    if ui
                                        .small_button(RichText::new("🗑").color(Color32::from_rgb(255, 100, 100)))
                                        .on_hover_text("Eliminar Deck Actual")
                                        .clicked()
                                    {
                                        let cur = self.engine.current_deck;
                                        self.engine.delete_deck(cur);
                                        for r in &mut self.thumb_uploaded_rev {
                                            for c in r {
                                                *c = 0;
                                            }
                                        }
                                    }

                                    ui.separator();
                                    let beat_phase = ((elapsed * (self.engine.bpm / 60.0)).fract() < 0.22) as u8;
                                    let led_col = if beat_phase == 1 {
                                        Color32::from_rgb(0, 255, 165)
                                    } else {
                                        Color32::from_rgb(25, 60, 45)
                                    };
                                    let (led_rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
                                    ui.painter().circle_filled(led_rect.center(), 4.5, led_col);

                                    ui.label(RichText::new("BPM").size(11.0).strong().color(Color32::from_rgb(0, 235, 170)));
                                    ui.add(egui::DragValue::new(&mut self.engine.bpm).speed(0.2).clamp_range(40.0..=240.0));
                                    if ui.small_button("/2").clicked() {
                                        self.engine.bpm = (self.engine.bpm * 0.5).max(40.0);
                                    }
                                    if ui.small_button("*2").clicked() {
                                        self.engine.bpm = (self.engine.bpm * 2.0).min(240.0);
                                    }
                                    if ui
                                        .button(
                                            RichText::new(format!("🥁 TAP ({})", self.tap_times.len()))
                                                .size(11.0)
                                                .strong()
                                                .color(Color32::from_rgb(255, 215, 50)),
                                        )
                                        .clicked()
                                    {
                                        self.register_bpm_tap();
                                    }
                                    if ui.small_button("RESYNC").clicked() {
                                        self.start_time = Instant::now();
                                    }
                                });
                            });
                    }

                    // ---------------- VISTA 2: DISEÑADOR DE CORTES / SLICES ----------------
                    CenterWorkspaceView::SliceDesignerView => {
                        let cur_layer_names = self.engine.current_layer_names();
                        egui::Frame::none()
                            .fill(Color32::from_rgb(18, 20, 25))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(0, 200, 150)))
                            .rounding(Rounding::same(5.0))
                            .inner_margin(egui::Margin::same(12.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.heading(RichText::new("📐 Diseñador de Cortes / Slices (Esquinas & Modo Malla)").color(Color32::from_rgb(0, 240, 175)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("✖ Volver a la Matriz").clicked() {
                                            self.center_view = CenterWorkspaceView::ClipMatrixGrid;
                                        }
                                    });
                                });
                                ui.separator();

                                ui.columns(2, |cols| {
                                    cols[0].horizontal(|ui| {
                                        if ui.button("➕ Añadir Nuevo Corte").clicked() {
                                            let idx = self.engine.slices.len() + 1;
                                            self.engine.slices.push(OutputSlice::new_rect(
                                                &format!("Slice {}", idx),
                                                SliceSource::Layer(1),
                                                0.25,
                                                0.25,
                                                0.50,
                                                0.50,
                                                [0, 210, 255],
                                            ));
                                            self.engine.selected_slice = self.engine.slices.len() - 1;
                                        }
                                        if self.engine.slices.len() > 1 && ui.button("🗑 Eliminar Corte").clicked() {
                                            self.engine.slices.remove(self.engine.selected_slice);
                                            self.engine.selected_slice = 0;
                                        }
                                    });
                                    cols[0].add_space(4.0);

                                    for i in 0..self.engine.slices.len() {
                                        let s_name = self.engine.slices[i].name.clone();
                                        let s_col = self.engine.slices[i].color_id();
                                        let s_src = self.engine.slices[i].source.label(&cur_layer_names);
                                        cols[0].horizontal(|ui| {
                                            ui.checkbox(&mut self.engine.slices[i].send_to_screen, "Activo");
                                            if ui
                                                .selectable_label(
                                                    self.engine.selected_slice == i,
                                                    RichText::new(format!("{}  [{}]", s_name, s_src)).color(s_col).strong(),
                                                )
                                                .clicked()
                                            {
                                                self.engine.selected_slice = i;
                                            }
                                        });
                                    }

                                    cols[0].separator();
                                    if let Some(slice) = self.engine.slices.get_mut(self.engine.selected_slice) {
                                        cols[0].horizontal(|ui| {
                                            ui.label("Nombre:");
                                            ui.text_edit_singleline(&mut slice.name);
                                        });
                                        cols[0].horizontal(|ui| {
                                            ui.label("Forma:");
                                            egui::ComboBox::from_id_source("slice_shape_cb")
                                                .selected_text(slice.shape.as_str())
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut slice.shape, SliceShape::Rectangle, SliceShape::Rectangle.as_str());
                                                    ui.selectable_value(&mut slice.shape, SliceShape::Circle, SliceShape::Circle.as_str());
                                                    ui.selectable_value(&mut slice.shape, SliceShape::Triangle, SliceShape::Triangle.as_str());
                                                    ui.selectable_value(&mut slice.shape, SliceShape::Hexagon, SliceShape::Hexagon.as_str());
                                                });
                                        });
                                        cols[0].horizontal(|ui| {
                                            ui.label(RichText::new("Capa Cargada:").strong().color(Color32::from_rgb(0, 235, 170)));
                                            let current_lbl = slice.source.label(&cur_layer_names);
                                            egui::ComboBox::from_id_source("slice_layer_source_cb")
                                                .selected_text(current_lbl)
                                                .show_ui(ui, |ui| {
                                                    for (idx, lname) in cur_layer_names.iter().enumerate() {
                                                        ui.selectable_value(&mut slice.source, SliceSource::Layer(idx), lname);
                                                    }
                                                    ui.separator();
                                                    ui.selectable_value(&mut slice.source, SliceSource::MasterComposition, "Composición Master");
                                                });
                                        });

                                        cols[0].horizontal(|ui| {
                                            ui.label("Ajuste Geométrico:");
                                            if ui.selectable_label(!slice.mesh_mode_enabled, "📐 4 Esquinas").clicked() {
                                                slice.mesh_mode_enabled = false;
                                            }
                                            if ui.selectable_label(slice.mesh_mode_enabled, "🕸 Modo Malla").clicked() {
                                                slice.mesh_mode_enabled = true;
                                                if slice.mesh_points.is_empty() {
                                                    slice.rebuild_mesh_from_corners();
                                                }
                                            }
                                        });

                                        if slice.mesh_mode_enabled {
                                            cols[0].horizontal(|ui| {
                                                if ui.selectable_label(slice.mesh_subdivisions == 3, "Malla 3x3").clicked() {
                                                    slice.mesh_subdivisions = 3;
                                                    slice.rebuild_mesh_from_corners();
                                                }
                                                if ui.selectable_label(slice.mesh_subdivisions == 4, "Malla 4x4").clicked() {
                                                    slice.mesh_subdivisions = 4;
                                                    slice.rebuild_mesh_from_corners();
                                                }
                                                if ui.small_button("Resetear Malla").clicked() {
                                                    slice.rebuild_mesh_from_corners();
                                                }
                                            });
                                        }

                                        cols[0].checkbox(
                                            &mut slice.preserve_aspect_ratio,
                                            "Auto-Ajustar Video Interno sin Deformarse (Aspect-Fill)",
                                        );

                                        let [mut bx, mut by, mut bw, mut bh] = slice.bounding_box();
                                        let mut changed = false;
                                        changed |= cols[0].add(egui::Slider::new(&mut bx, 0.0..=0.90).text("Pos X")).changed();
                                        changed |= cols[0].add(egui::Slider::new(&mut by, 0.0..=0.90).text("Pos Y")).changed();
                                        changed |= cols[0].add(egui::Slider::new(&mut bw, 0.05..=1.0).text("Ancho")).changed();
                                        changed |= cols[0].add(egui::Slider::new(&mut bh, 0.05..=1.0).text("Alto")).changed();
                                        if changed {
                                            slice.set_from_rect(bx, by, bw, bh);
                                        }
                                    }

                                    cols[1].label(RichText::new("Lienzo Interactivo de Mapeo:").strong());
                                    let c_w = cols[1].available_width().max(380.0);
                                    let c_h = (c_w * 9.0 / 16.0).clamp(220.0, 360.0);
                                    let (canvas_rect, canvas_resp) = cols[1].allocate_exact_size(Vec2::new(c_w, c_h), Sense::click_and_drag());
                                    let c_painter = cols[1].painter_at(canvas_rect);
                                    c_painter.rect_filled(canvas_rect, 4.0, Color32::BLACK);

                                    for (idx, slice) in self.engine.slices.iter().enumerate() {
                                        if !slice.send_to_screen {
                                            continue;
                                        }
                                        self.paint_slice_on_rect(
                                            &c_painter,
                                            canvas_rect,
                                            slice,
                                            true,
                                            idx == self.engine.selected_slice,
                                            false,
                                        );
                                    }

                                    let mouse_pos = canvas_resp.interact_pointer_pos();
                                    if canvas_resp.drag_started() {
                                        if let Some(mp) = mouse_pos {
                                            let sel = self.engine.selected_slice;
                                            if let Some(slice) = self.engine.slices.get(sel) {
                                                let mut found = None;
                                                if slice.mesh_mode_enabled {
                                                    for (m_idx, pt) in slice.mesh_points.iter().enumerate() {
                                                        let sp = Pos2::new(
                                                            canvas_rect.min.x + pt[0] * canvas_rect.width(),
                                                            canvas_rect.min.y + pt[1] * canvas_rect.height(),
                                                        );
                                                        if sp.distance(mp) < 16.0 {
                                                            found = Some(DragHandle::MeshNode(sel, m_idx));
                                                            break;
                                                        }
                                                    }
                                                } else {
                                                    for (c_idx, pt) in slice.corner_points.iter().enumerate() {
                                                        let sp = Pos2::new(
                                                            canvas_rect.min.x + pt[0] * canvas_rect.width(),
                                                            canvas_rect.min.y + pt[1] * canvas_rect.height(),
                                                        );
                                                        if sp.distance(mp) < 18.0 {
                                                            found = Some(DragHandle::Corner(sel, c_idx));
                                                            break;
                                                        }
                                                    }
                                                }
                                                self.active_drag = found.or(Some(DragHandle::WholeSlice(sel)));
                                            }
                                        }
                                    }

                                    if canvas_resp.dragged() {
                                        if let (Some(drag), Some(mp)) = (self.active_drag, mouse_pos) {
                                            let nx = ((mp.x - canvas_rect.min.x) / canvas_rect.width()).clamp(0.0, 1.0);
                                            let ny = ((mp.y - canvas_rect.min.y) / canvas_rect.height()).clamp(0.0, 1.0);
                                            let delta = canvas_resp.drag_delta();
                                            let dx = delta.x / canvas_rect.width();
                                            let dy = delta.y / canvas_rect.height();

                                            match drag {
                                                DragHandle::Corner(s_idx, c_idx) => {
                                                    if let Some(slice) = self.engine.slices.get_mut(s_idx) {
                                                        slice.corner_points[c_idx] = [nx, ny];
                                                        slice.rebuild_mesh_from_corners();
                                                    }
                                                }
                                                DragHandle::MeshNode(s_idx, m_idx) => {
                                                    if let Some(slice) = self.engine.slices.get_mut(s_idx) {
                                                        if let Some(pt) = slice.mesh_points.get_mut(m_idx) {
                                                            *pt = [nx, ny];
                                                        }
                                                    }
                                                }
                                                DragHandle::WholeSlice(s_idx) => {
                                                    if let Some(slice) = self.engine.slices.get_mut(s_idx) {
                                                        for cp in &mut slice.corner_points {
                                                            cp[0] = (cp[0] + dx).clamp(0.0, 1.0);
                                                            cp[1] = (cp[1] + dy).clamp(0.0, 1.0);
                                                        }
                                                        for mp_node in &mut slice.mesh_points {
                                                            mp_node[0] = (mp_node[0] + dx).clamp(0.0, 1.0);
                                                            mp_node[1] = (mp_node[1] + dy).clamp(0.0, 1.0);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if canvas_resp.drag_stopped() {
                                        self.active_drag = None;
                                    }
                                });
                            });
                    }

                    // ---------------- VISTA 3: CONVERSOR .MP4 -> .MOV (DXV3) ----------------
                    CenterWorkspaceView::Dxv3ConverterView => {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(18, 20, 25))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(0, 200, 150)))
                            .rounding(Rounding::same(5.0))
                            .inner_margin(egui::Margin::same(16.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.heading(RichText::new("🔄 Conversor .MP4 a .MOV (DXV3)").color(Color32::from_rgb(0, 240, 175)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("✖ Volver a la Matriz").clicked() {
                                            self.center_view = CenterWorkspaceView::ClipMatrixGrid;
                                        }
                                    });
                                });
                                ui.separator();
                                ui.add_space(8.0);

                                egui::Frame::none()
                                    .fill(Color32::from_rgb(13, 15, 19))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(36, 40, 50)))
                                    .rounding(Rounding::same(4.0))
                                    .inner_margin(egui::Margin::same(12.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            if ui.button("📂 1. Seleccionar Video (.MP4 / .MKV / .AVI)...").clicked() {
                                                if let Some(p) = rfd::FileDialog::new()
                                                    .add_filter("Videos", &["mp4", "mkv", "avi", "webm", "mov"])
                                                    .pick_file()
                                                {
                                                    let mut out_p = p.clone();
                                                    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                                                    out_p.set_file_name(format!("{}_DXV3.mov", stem));
                                                    self.engine.converter.input_path = Some(p);
                                                    self.engine.converter.output_path = Some(out_p);
                                                }
                                            }
                                            if let Some(in_p) = &self.engine.converter.input_path {
                                                ui.label(RichText::new(in_p.display().to_string()).color(Color32::from_rgb(255, 210, 50)).strong());
                                            }
                                        });

                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            if ui.button("💾 2. Destino (.MOV DXV3)...").clicked() {
                                                if let Some(out_p) = rfd::FileDialog::new()
                                                    .set_file_name("clip_DXV3.mov")
                                                    .add_filter("QuickTime DXV3 (.mov)", &["mov"])
                                                    .save_file()
                                                {
                                                    self.engine.converter.output_path = Some(out_p);
                                                }
                                            }
                                            if let Some(out_p) = &self.engine.converter.output_path {
                                                ui.label(RichText::new(out_p.display().to_string()).color(Color32::from_rgb(0, 235, 175)));
                                            }
                                        });

                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Perfil DXV3:");
                                            egui::ComboBox::from_id_source("dxv3_profile_cb")
                                                .selected_text(self.engine.converter.profile.label())
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(
                                                        &mut self.engine.converter.profile,
                                                        Dxv3ConvertProfile::NormalNoAlpha,
                                                        Dxv3ConvertProfile::NormalNoAlpha.label(),
                                                    );
                                                    ui.selectable_value(
                                                        &mut self.engine.converter.profile,
                                                        Dxv3ConvertProfile::HighWithAlpha,
                                                        Dxv3ConvertProfile::HighWithAlpha.label(),
                                                    );
                                                });
                                            ui.separator();
                                            ui.label("FPS:");
                                            ui.selectable_value(&mut self.engine.converter.target_fps, 30, "30 FPS");
                                            ui.selectable_value(&mut self.engine.converter.target_fps, 60, "60 FPS");
                                        });

                                        ui.add_space(10.0);
                                        let is_converting = self.engine.converter.is_converting.load(Ordering::Relaxed);
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add_enabled(
                                                    !is_converting && self.engine.converter.input_path.is_some(),
                                                    egui::Button::new(
                                                        RichText::new("⚡ CONVERTIR AHORA A .MOV (DXV3)")
                                                            .size(13.0)
                                                            .strong()
                                                            .color(Color32::BLACK),
                                                    )
                                                    .fill(Color32::from_rgb(0, 235, 170)),
                                                )
                                                .clicked()
                                            {
                                                self.start_mp4_to_mov_dxv3_conversion();
                                            }

                                            if let Ok(done_opt) = self.engine.converter.last_converted_file.try_lock() {
                                                if let Some(conv_path) = done_opt.clone() {
                                                    if ui.button("📥 Cargar en el Bloque Seleccionado").clicked() {
                                                        if let Some((pl, pc)) = self.engine.preview_target {
                                                            let deck = self.engine.current_deck;
                                                            let (iw, ih) = (self.engine.pool.width, self.engine.pool.height);
                                                            let fps = self.engine.target_fps;
                                                            self.engine.decks[deck].grid[pl][pc] =
                                                                ClipBlock::load_from_path(conv_path, iw, ih, fps);
                                                            self.thumb_textures[pl][pc] = None;
                                                            self.thumb_uploaded_rev[pl][pc] = 0;
                                                            self.center_view = CenterWorkspaceView::ClipMatrixGrid;
                                                        }
                                                    }
                                                }
                                            }
                                        });

                                        ui.add_space(8.0);
                                        if let Ok(msg) = self.engine.converter.status_message.try_lock() {
                                            ui.label(RichText::new(msg.clone()).size(11.5).color(Color32::from_rgb(0, 215, 255)));
                                        }
                                    });
                            });
                    }

                    // ---------------- VISTA 4: SALIDA, RESOLUCIÓN DEL CANVAS Y FPS ----------------
                    CenterWorkspaceView::DisplayRoutingView => {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(18, 20, 25))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(0, 200, 150)))
                            .rounding(Rounding::same(5.0))
                            .inner_margin(egui::Margin::same(16.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.heading(RichText::new("🖥 Configuración de Salida, Resolución de Canvas y FPS").color(Color32::from_rgb(0, 240, 175)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("✖ Volver a la Matriz").clicked() {
                                            self.center_view = CenterWorkspaceView::ClipMatrixGrid;
                                        }
                                        if ui.button("🔄 Escanear Monitores").clicked() {
                                            self.engine.monitors = WindowsMonitor::enumerate_windows_displays();
                                        }
                                    });
                                });
                                ui.separator();
                                ui.add_space(6.0);

                                ui.label(RichText::new("1. Resolución de Salida del Canvas y Cuadros por Segundo (FPS):").strong().color(Color32::YELLOW));
                                ui.horizontal(|ui| {
                                    let cur_res = self.engine.output_resolution;
                                    let cur_fps = self.engine.target_fps;
                                    for (res, label) in [
                                        ((1280, 720), "1280x720 (HD)"),
                                        ((1920, 1080), "1920x1080 (Full HD 1080p)"),
                                        ((2560, 1440), "2560x1440 (2K QHD)"),
                                        ((3840, 2160), "3840x2160 (4K UHD)"),
                                    ] {
                                        if ui.selectable_label(cur_res == res, label).clicked() {
                                            self.engine.set_output_resolution_and_fps(res.0, res.1, cur_fps);
                                        }
                                    }
                                    ui.separator();
                                    for fps in [30u32, 50, 60, 120] {
                                        if ui.selectable_label(cur_fps == fps, format!("{} FPS", fps)).clicked() {
                                            self.engine.set_output_resolution_and_fps(cur_res.0, cur_res.1, fps);
                                        }
                                    }
                                });

                                ui.add_space(10.0);
                                ui.separator();
                                ui.label(RichText::new("2. Pantalla Física de Destino en Windows:").strong().color(Color32::from_rgb(0, 235, 175)));
                                for m_idx in 0..self.engine.monitors.len() {
                                    let lbl = self.engine.monitors[m_idx].display_label();
                                    ui.radio_value(&mut self.engine.selected_monitor_idx, m_idx, RichText::new(lbl).size(12.5));
                                }

                                ui.add_space(10.0);
                                ui.separator();
                                if let Some(m) = self.engine.monitors.get_mut(self.engine.selected_monitor_idx) {
                                    ui.label(RichText::new("Ajuste de Coordenadas de la Pantalla Seleccionada:").strong());
                                    ui.horizontal(|ui| {
                                        ui.label("Pos X:");
                                        ui.add(egui::DragValue::new(&mut m.x).speed(10.0));
                                        ui.label("Pos Y:");
                                        ui.add(egui::DragValue::new(&mut m.y).speed(10.0));
                                        ui.label("Ancho (px):");
                                        ui.add(egui::DragValue::new(&mut m.width).speed(10.0));
                                        ui.label("Alto (px):");
                                        ui.add(egui::DragValue::new(&mut m.height).speed(10.0));
                                    });
                                }
                            });
                    }
                }
            });

        // ================= MODAL DE CONFIRMACIÓN AL CERRAR =================
        if self.show_exit_modal {
            egui::Window::new("🚪 Cerrar o Minimizar OSVJ")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .default_size([440.0, 210.0])
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("¿Qué deseas hacer con la sesión actual de OSVJ?").size(14.5).strong());
                    ui.add_space(8.0);

                    ui.checkbox(
                        &mut self.backup_before_exit,
                        RichText::new("⏳ Realizar un respaldo (BackUp) de todo lo hecho hasta el momento").color(Color32::from_rgb(0, 235, 175)),
                    );

                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                RichText::new("🚪 Salir a Windows")
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::from_rgb(255, 110, 110)),
                            )
                            .clicked()
                        {
                            if self.backup_before_exit {
                                let target = self.current_session_path.clone();
                                self.perform_backup_now(target);
                            }
                            for l in &mut self.engine.layers {
                                if let Some(c) = &l.playing_clip {
                                    c.stop_stream_if_any();
                                }
                            }
                            self.allow_immediate_close = true;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }

                        if ui
                            .button(
                                RichText::new("🗕 Minimizar a la Barra")
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::from_rgb(0, 230, 255)),
                            )
                            .clicked()
                        {
                            if self.backup_before_exit {
                                let target = self.current_session_path.clone();
                                self.perform_backup_now(target);
                            }
                            self.show_exit_modal = false;
                            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                        }

                        if ui.button(RichText::new("✖ Cancelar").size(13.0)).clicked() {
                            self.show_exit_modal = false;
                        }
                    });
                });
        }

        // ================= SPLASH SCREEN INICIAL DE OSVJ =================
        if self.show_splash_screen {
            egui::Area::new(egui::Id::new("osvj_fl_style_splash"))
                .order(egui::Order::Foreground)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    let (splash_rect, resp) = ui.allocate_exact_size(Vec2::new(560.0, 360.0), Sense::click());
                    let painter = ui.painter_at(splash_rect);

                    painter.rect_filled(splash_rect, 14.0, Color32::from_rgba_unmultiplied(12, 14, 19, 250));
                    painter.rect_stroke(splash_rect, 14.0, Stroke::new(2.0, Color32::from_rgb(0, 235, 175)));

                    let logo_center = Pos2::new(splash_rect.center().x, splash_rect.min.y + 115.0);
                    Self::paint_osvj_vector_logo(&painter, logo_center, 58.0, elapsed);

                    painter.text(
                        Pos2::new(splash_rect.center().x, splash_rect.min.y + 205.0),
                        Align2::CENTER_CENTER,
                        "OSVJ",
                        FontId::proportional(42.0),
                        Color32::WHITE,
                    );
                    painter.text(
                        Pos2::new(splash_rect.center().x, splash_rect.min.y + 238.0),
                        Align2::CENTER_CENTER,
                        "OpenSourceVJ · Ultra-Low CPU & GPU Visual Projection Engine",
                        FontId::proportional(14.5),
                        Color32::from_rgb(0, 235, 175),
                    );

                    let prog = if self.splash_opened_manually {
                        1.0
                    } else {
                        (elapsed / 3.0).clamp(0.0, 1.0)
                    };
                    let stage_txt = if prog < 0.35 {
                        "Inicializando WGPU High-Performance Pipeline..."
                    } else if prog < 0.70 {
                        "Activando On-Demand DXV3 Streamer & Auto-BackUp..."
                    } else {
                        "OSVJ v1.2 Listo — Haz clic para entrar al estudio"
                    };

                    let bar_rect = Rect::from_center_size(
                        Pos2::new(splash_rect.center().x, splash_rect.min.y + 292.0),
                        Vec2::new(440.0, 8.0),
                    );
                    painter.rect_filled(bar_rect, 4.0, Color32::from_rgb(28, 32, 40));
                    let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(bar_rect.width() * prog, bar_rect.height()));
                    painter.rect_filled(fill_rect, 4.0, Color32::from_rgb(0, 240, 175));

                    painter.text(
                        Pos2::new(splash_rect.center().x, splash_rect.min.y + 318.0),
                        Align2::CENTER_CENTER,
                        stage_txt,
                        FontId::monospace(11.5),
                        Color32::LIGHT_GRAY,
                    );

                    if resp.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Space)) {
                        self.show_splash_screen = false;
                        self.splash_opened_manually = false;
                    }
                });
        }

        // ================= SALIDA EXTERNA A MÁXIMA RESOLUCIÓN DEL MONITOR ELEGIDO =================
        if self.engine.selected_monitor_idx > 0 {
            if let Some(mon) = self.engine.monitors.get(self.engine.selected_monitor_idx).cloned() {
                let slices_clone = self.engine.slices.clone();
                let master_id = self.master_tex.as_ref().map(|t| t.id());
                let layer_ids: Vec<Option<TextureId>> = self
                    .layer_textures
                    .iter()
                    .map(|opt| opt.as_ref().map(|t| t.id()))
                    .collect();

                let target_pos = Pos2::new(mon.x as f32, mon.y as f32);
                let target_size = Vec2::new(mon.width as f32, mon.height as f32);
                let force_reposition = self.last_applied_monitor != self.engine.selected_monitor_idx;
                self.last_applied_monitor = self.engine.selected_monitor_idx;

                let mut revert_to_main = false;

                ctx.show_viewport_immediate(
                    egui::ViewportId::from_hash_of(format!("osvj_ext_display_{}", mon.index)),
                    egui::ViewportBuilder::default()
                        .with_title(format!("OSVJ Live Output - {}", mon.device_name))
                        .with_position(target_pos)
                        .with_inner_size(target_size)
                        .with_decorations(false)
                        .with_always_on_top(),
                    |ctx, _class| {
                        if force_reposition {
                            ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
                            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(target_pos));
                            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target_size));
                        }
                        if ctx.input(|i| i.viewport().close_requested()) {
                            revert_to_main = true;
                        }
                        egui::CentralPanel::default()
                            .frame(egui::Frame::none().fill(Color32::BLACK))
                            .show(ctx, |ui| {
                                let out_rect = ui.max_rect();
                                let painter = ui.painter_at(out_rect);

                                for slice in &slices_clone {
                                    if !slice.send_to_screen {
                                        continue;
                                    }
                                    let tex_opt = match slice.source {
                                        SliceSource::MasterComposition => master_id,
                                        SliceSource::Layer(idx) => layer_ids.get(idx).copied().flatten(),
                                    };
                                    let Some(tex_id) = tex_opt else { continue };

                                    let (verts, tris) = slice.build_geometry();
                                    let mut mesh = egui::Mesh::with_texture(tex_id);
                                    for (pos_norm, uv_norm) in verts {
                                        mesh.vertices.push(egui::epaint::Vertex {
                                            pos: Pos2::new(
                                                out_rect.min.x + pos_norm[0] * out_rect.width(),
                                                out_rect.min.y + pos_norm[1] * out_rect.height(),
                                            ),
                                            uv: Pos2::new(uv_norm[0], uv_norm[1]),
                                            color: Color32::WHITE,
                                        });
                                    }
                                    for tri in tris {
                                        mesh.add_triangle(tri[0] as u32, tri[1] as u32, tri[2] as u32);
                                    }
                                    painter.add(Shape::Mesh(mesh));
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                    revert_to_main = true;
                                }
                            });
                    },
                );

                if revert_to_main {
                    self.engine.selected_monitor_idx = 0;
                    self.last_applied_monitor = 0;
                }
            }
        } else {
            self.last_applied_monitor = 0;
        }
    }
}
