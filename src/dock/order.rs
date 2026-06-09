//! Панель ордера — правая колонка контейнера (рядом с чартом+стаканом). Пока
//! только подпись; кнопки/форму ввода добавим позже. При откреплении чарта в окно
//! ордер НЕ выносится (часть докнутого контейнера).

pub fn show(ctx: &egui::Context) {
    egui::SidePanel::right("order")
        .exact_width(150.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(t!("order.title")).weak());
        });
}
