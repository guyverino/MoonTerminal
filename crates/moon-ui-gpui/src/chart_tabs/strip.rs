//! Рендер полоски чарт-вкладок (`impl Render for ChartTabs`): сам таб-стрип (Main + AddToChart),
//! кнопки «собрать окна» (▦) и настроек раскладки (⚙ + canvas-проба её rect), плюс активная панель
//! ниже. Логика вкладок/синхронизации — в [`super`] (mod.rs), выносные окна — в [`super::windows`].

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonRect, MoonTabItem, MoonTabStrip, v_flex,
};

use super::{CHART_TAB_STRIP_H, ChartTabs, Tab};

impl Render for ChartTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Снимок вкладок — чтобы callbacks не держали borrow self.add. (Tab, label, count для
        // ширины, unread для бейджа, detachable.)
        let mut tabs: Vec<(Tab, String, usize, usize, bool)> =
            vec![(Tab::Main, "Main".to_string(), 0, 0, false)];
        tabs.extend(self.add.iter().map(|(n, bucket, panel)| {
            let count = panel.read(cx).pane_count(cx);
            let seen = self.seen.get(&(*n, bucket.clone())).copied().unwrap_or(0);
            (
                Tab::Add(*n, bucket.clone()),
                self.add_label(*n, bucket, cx),
                count,
                count.saturating_sub(seen),
                true,
            )
        }));
        let tab_keys = Rc::new(
            tabs.iter()
                .map(|(tab, _, _, _, _)| tab.clone())
                .collect::<Vec<_>>(),
        );
        let items = tabs
            .iter()
            .map(|(tab, label, _count, unread, detachable)| {
                let width = (label.chars().count() as f32 * 7.0
                    + if *unread > 0 { 38.0 } else { 28.0 }
                    + if *detachable { 20.0 } else { 0.0 })
                .clamp(72.0, 168.0);
                let mut item = MoonTabItem::new(label.clone())
                    .width(width)
                    .selected(self.active == *tab)
                    .closable(*detachable);
                if *unread > 0 {
                    item = item.badge(unread.to_string());
                }
                item
            })
            .collect::<Vec<_>>();
        let view = cx.entity();
        // MoonTabStrip рисует ВСЕ табы абсолютно и режет по `overflow_hidden` ПО СВОИМ
        // bounds. Без явных bounds его root схлопывается в 0×0 → полоска невидима, а чарт
        // (flex_1 ниже) забирает всю высоту (ровно баг «график есть, вкладок нет»). Даём
        // ширину окна (контейнер ниже обрежет до ширины панели) и фикс. высоту полосы.
        let strip_w = f32::from(window.viewport_size().width).max(1.0);
        let strip = MoonTabStrip::new("chart-tabs-strip")
            .padding_left(8.0)
            .gap(4.0)
            .bounds(MoonRect::new(0.0, 0.0, strip_w, CHART_TAB_STRIP_H))
            .items(items)
            .on_click({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if !matches!(tab_id, Tab::Main) && event.click_count() >= 2 {
                            this.detach(tab_id, cx);
                        } else if matches!(tab_id, Tab::Main)
                            || this
                                .add
                                .iter()
                                .any(|(n, c, _)| Tab::Add(*n, c.clone()) == tab_id)
                        {
                            if this.active != tab_id {
                                this.active = tab_id;
                                this.sync_seen_for_active(cx);
                                this.sync_active_scale(cx);
                                this.sync_inactive_chart_visibility(cx);
                                this.persist_scales(cx);
                                cx.notify();
                            }
                        }
                    });
                }
            })
            .on_close({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, _event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    if matches!(tab_id, Tab::Main) {
                        return;
                    }
                    view.update(app, |this, cx| {
                        this.add
                            .retain(|(n, c, _)| Tab::Add(*n, c.clone()) != tab_id);
                        if this.active == tab_id {
                            this.active = Tab::Main;
                        }
                        this.sync_seen_for_active(cx);
                        this.sync_active_scale(cx);
                        this.sync_inactive_chart_visibility(cx);
                        this.persist_scales(cx);
                        cx.notify();
                    });
                }
            });

        // Кнопка «собрать окна» — справа в полосе вкладок, ТОЛЬКО если у группы есть откреп-окна.
        // Восстанавливает/показывает/возвращает на экран окна чартов, если они свёрнуты/спрятаны/
        // уехали за пределы экранов (они независимы и не ходят за Main).
        let detached_count = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == self.group)
            .count();
        let gather_btn = (detached_count > 0).then(|| {
            let entity = cx.entity();
            div().absolute().right(px(34.0)).top(px(4.0)).child(
                MoonButton::new("chart-gather-windows")
                    .label("▦")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.gather_windows(cx));
                    })
                    .render(),
            )
        });

        // Кнопка настроек раскладки активной вкладки (⚙). canvas-проба снимает её оконный rect в
        // `settings_btn_rect` — по нему попап привязывается правым краём к правому краю кнопки.
        let popup_open = self.layout_popup.is_some();
        let settings_btn = {
            let entity = cx.entity();
            let rect_cell = self.settings_btn_rect.clone();
            div()
                .absolute()
                .right(px(6.0))
                .top(px(4.0))
                .child(
                    MoonButton::new("chart-layout-settings")
                        .label("⚙")
                        .size(MoonButtonSize::Micro)
                        .variant(if popup_open {
                            MoonButtonVariant::Blue
                        } else {
                            MoonButtonVariant::Ghost
                        })
                        .selected(popup_open)
                        .on_click(move |_, window, app| {
                            entity.update(app, |this, cx| this.toggle_layout_popup(window, cx));
                        })
                        .render(),
                )
                .child(
                    canvas(
                        move |bounds, _, _| bounds,
                        move |bounds, _, _w, _cx| rect_cell.set(Some(bounds)),
                    )
                    .absolute()
                    .size_full(),
                )
        };

        v_flex()
            .size_full()
            .relative()
            .child(
                div()
                    .h(px(CHART_TAB_STRIP_H))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip)
                    .children(gather_btn)
                    .child(settings_btn),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.active_element()),
            )
    }
}
