//! Панель ордера — пока только подпись контейнера. Кнопки/форму ввода добавим
//! позже, когда определимся с составом и видом торгового интерфейса.

pub fn show(ctx: &egui::Context) {
    egui::SidePanel::left("order")
        .exact_width(150.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(t!("order.title")).weak());
        });
}
