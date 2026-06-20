//! Поля-списки (источник/тип) и меню сортировки/фильтра панели «Ордера».

use super::*;

impl OrdersPanel {
    /// Поле-список источника (Все ядра + ядра группы) — порт egui ComboBox.
    pub(super) fn source_combo(
        &self,
        cores: &[(CoreId, String)],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let cur = match self.view.source {
            OrdersSource::All => "Все ядра".to_string(),
            OrdersSource::Core(id) => cores
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "Все ядра".into()),
        };
        let view = cx.entity();
        let mut menu = MoonDropdown::new("orders-source")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(118.0)
            .menu_width(160.0)
            .menu_size(MoonMenuSize::Compact)
            .item(
                MoonMenuItem::with_key("all", "Все ядра")
                    .checked(matches!(self.view.source, OrdersSource::All))
                    .on_click({
                        let view = view.clone();
                        move |_, _, app| Self::mutate(&view, app, |v| v.source = OrdersSource::All)
                    }),
            );
        for (id, name) in cores {
            let id = *id;
            let selected = matches!(self.view.source, OrdersSource::Core(cur) if cur == id);
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("core-{id}"), name.clone())
                    .checked(selected)
                    .on_click(move |_, _, app| {
                        Self::mutate(&view, app, |v| v.source = OrdersSource::Core(id))
                    }),
            );
        }
        menu
    }

    /// Поле-список типа ордеров (Все / Реальные / Эмуляторные).
    pub(super) fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.view.kind {
            OrderKind::All => "Все",
            OrderKind::Real => "Реальные",
            OrderKind::Emu => "Эмуляторные",
        };
        let view = cx.entity();
        let mut menu = MoonDropdown::new("orders-kind")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(102.0)
            .menu_width(138.0)
            .menu_size(MoonMenuSize::Compact);
        for (k, label) in [
            (OrderKind::All, "Все"),
            (OrderKind::Real, "Реальные"),
            (OrderKind::Emu, "Эмуляторные"),
        ] {
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("kind-{label}"), label)
                    .checked(self.view.kind == k)
                    .on_click(move |_, _, app| Self::mutate(&view, app, |v| v.kind = k)),
            );
        }
        menu
    }

    /// Меню сортировки/фильтра (порт ПКМ-меню egui): фильтр текущего маркета + две
    /// тогл-группы сортировки. В GPUI — попап-кнопка (PopupMenu основан на Action).
    pub(super) fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let v = view.clone();
        let mut menu = MoonDropdown::new("orders-sort")
            .label("⚙")
            .trigger_variant(MoonButtonVariant::Ghost)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(34.0)
            .menu_width(220.0)
            .menu_size(MoonMenuSize::Normal)
            .item(
                MoonMenuItem::with_key("m-onlycur", "Только ордера текущего маркета")
                    .checked(cur.only_current_market)
                    .on_click(move |_, _, app| {
                        Self::mutate(&v, app, |s| s.only_current_market = true)
                    }),
            );
        let v = view.clone();
        menu = menu
            .item(
                MoonMenuItem::with_key("m-showall", "Показать все")
                    .checked(!cur.only_current_market)
                    .on_click(move |_, _, app| {
                        Self::mutate(&v, app, |s| s.only_current_market = false)
                    }),
            )
            .item(MoonMenuItem::separator());
        for (variant, label, id) in [
            (PrimarySort::SellFirst, "Sell первые", "m-sell"),
            (PrimarySort::BuyFirst, "Buy первые", "m-buy"),
            (PrimarySort::Creation, "По созданию ордера", "m-creation"),
        ] {
            let v = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(id, label)
                    .checked(cur.primary == variant)
                    .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.primary = variant)),
            );
        }
        let v = view.clone();
        menu = menu.item(MoonMenuItem::separator()).item(
            MoonMenuItem::with_key("m-new", "Новые первые")
                .checked(cur.newest_first)
                .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.newest_first = true)),
        );
        let v = view;
        menu.item(
            MoonMenuItem::with_key("m-old", "Старые первые")
                .checked(!cur.newest_first)
                .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.newest_first = false)),
        )
    }
}
