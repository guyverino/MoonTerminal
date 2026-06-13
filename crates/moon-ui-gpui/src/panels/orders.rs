//! Панель «Ордера» — виртуализированная таблица ордеров группы (gpui-component Table).
//! Дренаж backend пересобирает строки. Группа хранится для персиста раскладки (dump).

use gpui::*;
use gpui_component::{
    dock::{Panel, PanelEvent, PanelState},
    table::{Table, TableState},
};

use crate::{collect_orders, Backend, OrdersDelegate};

/// Панель «Ордера» — виртуализированная таблица ордеров группы.
pub struct OrdersPanel {
    orders: Entity<TableState<OrdersDelegate>>,
    /// Группа окна — нужна для персиста раскладки (dump → docks.json).
    group: String,
    focus: FocusHandle,
}

impl OrdersPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let orders = cx.new(|cx| TableState::new(OrdersDelegate::new(), window, cx));
        // Дренаж backend → пересобрать строки таблицы.
        let g = group.clone();
        let orders_h = orders.clone();
        cx.observe(&backend, move |_this, backend, cx| {
            let rows = collect_orders(backend.read(cx), &g);
            orders_h.update(cx, |st, cx| {
                st.delegate_mut().rows = rows;
                cx.notify();
            });
        })
        .detach();
        Self { orders, group, focus: cx.focus_handle() }
    }
}

impl EventEmitter<PanelEvent> for OrdersPanel {}
impl Focusable for OrdersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrdersPanel {
    fn panel_name(&self) -> &'static str {
        "Orders"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Ордера")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Orders", &self.group)
    }
}
impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("orders-panel")
            .size_full()
            .track_focus(&self.focus)
            .child(Table::new(&self.orders))
    }
}
