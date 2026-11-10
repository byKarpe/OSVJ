#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Pos2, RichText, Rounding, Sense, Shape, Stroke, Vec2};
use image::ImageEncoder;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const OSVJ_EXE_BYTES: &[u8] = include_bytes!("../target/release/OSVJ.exe");

fn generate_ico_file(path: &Path) -> Result<(), String> {
    let size = 256u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 * 0.5;
    let r_hex = size as f32 * 0.44;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = ((y * size + x) * 4) as usize;

            if dist <= r_hex {
                let border = (r_hex - dist) < (size as f32 * 0.045);
                let in_prism = dx > -r_hex * 0.25
                    && dx < r_hex * 0.55
                    && dy.abs() < (r_hex * 0.55 - dx) * 0.62;

                let (r, g, b, a) = if border {
                    (0, 240, 175, 255)
                } else if in_prism {
                    (0, 225, 165, 255)
                } else {
                    (14, 18, 24, 255)
                };
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = a;
            }
        }
    }

    let mut png_bytes: Vec<u8> = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png_bytes)
        .write_image(&rgba, size, size, image::ColorType::Rgba8)
        .map_err(|e| e.to_string())?;

    let mut ico_bytes: Vec<u8> = Vec::with_capacity(22 + png_bytes.len());
    ico_bytes.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    ico_bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0]);
    ico_bytes.extend_from_slice(&(png_bytes.len() as u32).to_le_bytes());
    ico_bytes.extend_from_slice(&22u32.to_le_bytes());
    ico_bytes.extend_from_slice(&png_bytes);

    fs::write(path, ico_bytes).map_err(|e| e.to_string())
}

fn create_windows_shortcut(target_exe: &Path, work_dir: &Path, icon_path: &Path, folder_env: &str) {
    let ps = format!(
        "$dir = [Environment]::GetFolderPath('{}'); $lnk = Join-Path $dir 'OSVJ - OpenSourceVJ.lnk'; $ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut($lnk); $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{}'; $s.Save()",
        folder_env,
        target_exe.display(),
        work_dir.display(),
        icon_path.display()
    );
    let mut cmd = Command::new("powershell");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let _ = cmd.args(["-NoProfile", "-Command", &ps]).status();
}

struct OsvjInstallerApp {
    install_dir: PathBuf,
    create_desktop_shortcut: bool,
    create_start_menu_shortcut: bool,
    installed_ok: bool,
    status_msg: String,
}

impl Default for OsvjInstallerApp {
    fn default() -> Self {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:\\".into());
        let install_dir = PathBuf::from(base).join("Programs").join("OSVJ");
        Self {
            install_dir,
            create_desktop_shortcut: true,
            create_start_menu_shortcut: true,
            installed_ok: false,
            status_msg: "Listo para instalar OSVJ v1.2 en tu sistema Windows.".into(),
        }
    }
}

impl OsvjInstallerApp {
    fn run_install(&mut self) {
        if let Err(e) = fs::create_dir_all(&self.install_dir) {
            self.status_msg = format!("Error al crear carpeta: {}", e);
            return;
        }

        let exe_path = self.install_dir.join("OSVJ.exe");
        if let Err(e) = fs::write(&exe_path, OSVJ_EXE_BYTES) {
            self.status_msg = format!("Error al escribir OSVJ.exe (cierra OSVJ si está abierto): {}", e);
            return;
        }

        let ico_path = self.install_dir.join("osvj.ico");
        let _ = generate_ico_file(&ico_path);

        if Path::new("ffmpeg.exe").exists() {
            let _ = fs::copy("ffmpeg.exe", self.install_dir.join("ffmpeg.exe"));
        }

        if self.create_desktop_shortcut {
            create_windows_shortcut(&exe_path, &self.install_dir, &ico_path, "Desktop");
        }

        if self.create_start_menu_shortcut {
            create_windows_shortcut(&exe_path, &self.install_dir, &ico_path, "Programs");
        }

        self.installed_ok = true;
        self.status_msg = format!(
            "✔ ¡OSVJ v1.2 instalado correctamente en {}!",
            self.install_dir.display()
        );
    }
}

impl eframe::App for OsvjInstallerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(13, 15, 20)).inner_margin(egui::Margin::same(22.0)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let (logo_rect, _) = ui.allocate_exact_size(Vec2::new(90.0, 90.0), Sense::hover());
                    let painter = ui.painter_at(logo_rect);
                    let center = logo_rect.center();
                    let r = 38.0_f32;

                    painter.circle_filled(center, r * 1.15, Color32::from_rgba_unmultiplied(0, 235, 175, 22));
                    let mut hex = Vec::new();
                    for i in 0..6 {
                        let a = (i as f32 / 6.0) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                        hex.push(Pos2::new(center.x + a.cos() * r, center.y + a.sin() * r));
                    }
                    painter.add(Shape::convex_polygon(
                        hex,
                        Color32::from_rgb(16, 20, 28),
                        Stroke::new(2.2_f32, Color32::from_rgb(0, 240, 175)),
                    ));
                    painter.add(Shape::convex_polygon(
                        vec![
                            Pos2::new(center.x - r * 0.22, center.y - r * 0.45),
                            Pos2::new(center.x + r * 0.52, center.y),
                            Pos2::new(center.x - r * 0.22, center.y + r * 0.45),
                        ],
                        Color32::from_rgb(0, 230, 170),
                        Stroke::new(1.2_f32, Color32::WHITE),
                    ));

                    ui.add_space(4.0);
                    ui.label(RichText::new("OSVJ · OpenSourceVJ v1.2").size(24.0).strong().color(Color32::WHITE));
                    ui.label(
                        RichText::new("Instalador Oficial para Windows (64-bit GPU WGPU)")
                            .size(12.5)
                            .color(Color32::from_rgb(0, 235, 175)),
                    );
                });

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label(RichText::new("Carpeta de Instalación:").strong());
                ui.horizontal(|ui| {
                    let mut dir_str = self.install_dir.display().to_string();
                    ui.add(egui::TextEdit::singleline(&mut dir_str).desired_width(370.0));
                    if ui.button("📂 Cambiar...").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.install_dir = folder.join("OSVJ");
                        }
                    }
                });

                ui.add_space(8.0);
                ui.checkbox(
                    &mut self.create_desktop_shortcut,
                    "🖥 Crear acceso directo en el Escritorio de Windows (con icono oficial)",
                );
                ui.checkbox(
                    &mut self.create_start_menu_shortcut,
                    "🚀 Agregar OSVJ al Menú Inicio de Windows",
                );

                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    if !self.installed_ok {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("⚡ INSTALAR OSVJ v1.2 AHORA")
                                        .size(15.0)
                                        .strong()
                                        .color(Color32::BLACK),
                                )
                                .fill(Color32::from_rgb(0, 235, 170))
                                .min_size(Vec2::new(280.0, 40.0)),
                            )
                            .clicked()
                        {
                            self.run_install();
                        }
                    } else {
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("🚀 INICIAR OSVJ AHORA")
                                            .size(14.0)
                                            .strong()
                                            .color(Color32::BLACK),
                                    )
                                    .fill(Color32::from_rgb(0, 240, 175))
                                    .min_size(Vec2::new(220.0, 38.0)),
                                )
                                .clicked()
                            {
                                let exe_path = self.install_dir.join("OSVJ.exe");
                                let _ = Command::new(&exe_path).current_dir(&self.install_dir).spawn();
                                std::process::exit(0);
                            }

                            if ui
                                .add(
                                    egui::Button::new(RichText::new("✖ Finalizar y Cerrar").size(13.5))
                                        .min_size(Vec2::new(140.0, 38.0)),
                                )
                                .clicked()
                            {
                                std::process::exit(0);
                            }
                        });
                    }
                });

                ui.add_space(12.0);
                ui.label(
                    RichText::new(&self.status_msg)
                        .size(12.0)
                        .color(if self.installed_ok {
                            Color32::from_rgb(0, 240, 170)
                        } else {
                            Color32::LIGHT_GRAY
                        }),
                );
            });
    }
}

fn main() -> eframe::Result<()> {
    let mut visuals = egui::Visuals::dark();
    visuals.window_rounding = Rounding::same(8.0);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 390.0])
            .with_resizable(false)
            .with_title("Instalador de OSVJ v1.2 — OpenSourceVJ"),
        ..Default::default()
    };

    eframe::run_native(
        "OSVJ Installer",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(visuals);
            Box::new(OsvjInstallerApp::default())
        }),
    )
}
