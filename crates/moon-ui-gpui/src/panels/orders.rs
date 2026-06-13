//! Панель «Ордера» — виртуализированная таблица ордеров группы (gpui-component Table).
//! Дренаж backend пересобирает строки. Группа хранится для персиста раскладки (dump).

use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState, PanelView, TabPanel},
    table::{Table, TableState},
};

use crate::detached::DetachedSpec;
use crate::{collect_orders, Backend, OrdersDelegate};

/// Панель «Ордера» — виртуализированная таблица ордеров группы.
pub struct OrdersPanel {
    /// Общий backend — для дренажа и пере-открытия панели в отдельном окне (detach).
    backend: Entity<Backend>,
    orders: Entity<TableState<OrdersDelegate>>,
    /// Группа окна — нужна для персиста раскладки (dump → docks.json) и detach.
    group: String,
    /// Таб-панель дока, в которой живёт эта панель (для самоудаления при откреплении).
    tab: Option<WeakEntity<TabPanel>>,
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
        Self { backend, orders, group, tab: None, focus: cx.focus_handle() }
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
    /// Запоминаем таб-панель-владельца — нужна, чтобы убрать себя из дока при откреплении.
    fn on_added_to(&mut self, tab_panel: WeakEntity<TabPanel>, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab = Some(tab_panel);
    }
    /// Кнопка «⧉»: убрать «Ордера» из дока + открыть в отдельном окне + записать спеку
    /// (персист → на старте восстановится отцепленной). Порт egui `open_detached`.
    fn toolbar_buttons(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let tab = self.tab.clone();
        let me = cx.entity().downgrade();
        Some(vec![Button::new("detach-orders")
            .ghost()
            .label("⧉")
            .tooltip("В отдельное окно")
            .on_click(move |_, window, app| {
                if let (Some(tab), Some(me)) = (tab.as_ref().and_then(|t| t.upgrade()), me.upgrade()) {
                    let arc: Arc<dyn PanelView> = Arc::new(me);
                    tab.update(app, |tp, cx| tp.remove_panel(arc, window, cx));
                }
                let spec = DetachedSpec::new(group.clone(), "Orders".to_string());
                crate::detached::spawn(app, &backend, &spec);
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            })])
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
