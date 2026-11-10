#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod app;

use eframe::egui::{self, Color32, Rounding, Stroke, Vec2};
use std::sync::Arc;

fn build_osvj_icon_rgba(size: u32) -> Vec<u8> {
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
    rgba
}

fn configure_osvj_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(5.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(8.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.slider_width = 110.0;

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(15, 16, 20);
    visuals.window_fill = Color32::from_rgb(19, 21, 26);
    visuals.extreme_bg_color = Color32::from_rgb(9, 10, 13);
    visuals.faint_bg_color = Color32::from_rgb(23, 25, 31);
    visuals.window_rounding = Rounding::same(6.0);
    visuals.menu_rounding = Rounding::same(4.0);

    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(20, 22, 27);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(35, 39, 48));
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(190, 196, 208));
    visuals.widgets.noninteractive.rounding = Rounding::same(3.0);

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(27, 30, 37);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(44, 49, 60));
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(215, 220, 230));
    visuals.widgets.inactive.rounding = Rounding::same(3.0);

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(36, 48, 58);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0, 235, 175));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.hovered.rounding = Rounding::same(3.0);

    visuals.widgets.active.bg_fill = Color32::from_rgb(0, 185, 135);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0, 255, 190));
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    visuals.widgets.active.rounding = Rounding::same(3.0);

    visuals.selection.bg_fill = Color32::from_rgb(0, 150, 110);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0, 240, 180));

    style.visuals = visuals;
    ctx.set_style(style);
}

fn main() -> eframe::Result<()> {
    std::env::set_var("WGPU_POWER_PREF", "high");

    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    wgpu_options.power_preference = wgpu::PowerPreference::HighPerformance;
    wgpu_options.supported_backends = wgpu::Backends::DX12 | wgpu::Backends::VULKAN | wgpu::Backends::PRIMARY;
    wgpu_options.device_descriptor = Arc::new(|_adapter| wgpu::DeviceDescriptor {
        label: Some("OSVJ High-Performance GPU Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
    });

    let icon_data = egui::IconData {
        rgba: build_osvj_icon_rgba(64),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1760.0, 990.0])
            .with_min_inner_size([1280.0, 720.0])
            .with_icon(Arc::new(icon_data))
            .with_title("OSVJ — OpenSourceVJ v1.2 | GPU Hardware Visual Engine"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        "OSVJ - OpenSourceVJ",
        options,
        Box::new(|cc| {
            configure_osvj_style(&cc.egui_ctx);
            Box::new(app::VjApp::new(cc))
        }),
    )
}
