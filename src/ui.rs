pub struct VjApp {}

impl Default for VjApp {
    fn default() -> Self {
        Self {}
    }
}

impl eframe::App for VjApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("VJ Engine - Resolume Clone");
            ui.label("El motor grafico, renderizado de slices y capas se conectaran aqui.");
        });
    }
}
