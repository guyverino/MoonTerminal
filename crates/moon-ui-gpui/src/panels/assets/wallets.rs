//! Нижняя часть окна «Активы»: 3 контейнера кошельков (Спот/Фьючерсы/Квартальные) с
//! монетами выбранного ядра, drag&drop переносом между ними и модальным диалогом
//! количества (дефолт — всё свободное).

use super::*;

/// Полезная нагрузка drag&drop переноса актива между кошельками.
#[derive(Clone)]
pub(super) struct AssetDrag {
    pub(super) core: CoreId,
    pub(super) asset: String,
    pub(super) from: WalletKind,
    /// Свободное количество монеты (дефолт для диалога — перенести всё).
    pub(super) free: f64,
}

/// Ожидающий подтверждения перенос (открыт диалог количества).
#[derive(Clone)]
pub(super) struct PendingTransfer {
    core: CoreId,
    asset: String,
    from: WalletKind,
    to: WalletKind,
    /// Свободное количество (максимум / дефолт).
    free: f64,
}

/// Превью под курсором при перетаскивании монеты.
struct AssetDragPreview {
    label: SharedString,
}

impl Render for AssetDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.blue))
            .text_color(rgb(p.text))
            .text_size(design::text_px(cx, 10.5))
            .font_family(design::mono())
            .child(self.label.clone())
    }
}

impl AssetsView {
    /// Секция кошельков ядра: заголовок (+ ↻ refresh) и 3 контейнера в ряд.
    pub(super) fn wallets_section(
        &self,
        b: &Backend,
        core: CoreId,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .h_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .child("Кошельки · перетащи монету между контейнерами"),
                    )
                    .child(
                        MoonButton::new("assets-refresh-transfer")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .label("↻")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.read(cx).session.refresh_transfer_assets(core);
                                cx.notify();
                            }))
                            .render(),
                    ),
            )
            .child(
                h_flex().w_full().flex_1().min_h(px(0.0)).children(
                    WalletKind::ALL
                        .into_iter()
                        .map(|kind| self.wallet_column(b, core, kind, cx)),
                ),
            )
    }

    /// Один контейнер кошелька (Спот/Фьючерсы/Квартальные): монеты (draggable) и
    /// drop-таргет. Бросок монеты из другого кошелька открывает диалог количества.
    fn wallet_column(
        &self,
        b: &Backend,
        core: CoreId,
        kind: WalletKind,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let all_items = b
            .session
            .store()
            .core(core)
            .map(|c| c.transfer_assets.wallet(kind).to_vec())
            .unwrap_or_default();
        // Фильтр >1 USDT (нет цены → скрыто), сортировка по убыванию стоимости.
        let mut items: Vec<&TransferAssetRow> = all_items
            .iter()
            .filter(|a| self.show_all || a.value_usdt > 1.0)
            .collect();
        items.sort_by(|a, b| {
            b.value_usdt
                .partial_cmp(&a.value_usdt)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut list = v_flex().w_full().gap_0().p(px(4.0));
        if items.is_empty() {
            list = list.child(
                div()
                    .px(design::ui_px(cx, 6.0))
                    .py(px(2.0))
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child("—"),
            );
        }
        for a in items {
            let drag = AssetDrag {
                core,
                asset: a.currency.clone(),
                from: kind,
                free: a.amount,
            };
            let preview_label: SharedString = format!("{} {}", a.currency, num(a.amount)).into();
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "coin-{core}-{}-{}",
                        kind.to_u8(),
                        a.currency
                    )))
                    .w_full()
                    .h(design::fit_h_px(cx, 26.0, 12.0, 6.0))
                    .px(design::ui_px(cx, 6.0))
                    .rounded(px(3.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_grab()
                    .text_color(rgb(p.text))
                    // Заметная подсветка строки при наведении (видно, что потащишь).
                    .hover(|s| s.bg(rgba(0x3b82f626)).border_color(rgb(p.blue)))
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.blue))
                            .child(a.currency.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_muted))
                            .child(num(a.amount)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_soft))
                            .child(money(a.value_usdt)),
                    )
                    .on_drag(drag, move |_d, _pos, _w, cx| {
                        cx.new(|_| AssetDragPreview {
                            label: preview_label.clone(),
                        })
                    }),
            );
        }

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .bg(rgb(p.shell_high))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text_soft))
                    .child(format!("{} ({})", kind.label(), all_items.len())),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "wallet-col-{core}-{}",
                        kind.to_u8()
                    )))
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .drag_over::<AssetDrag>(|s, _drag, _w, _cx| s.bg(rgba(0x3b82f622)))
                    .on_drop(cx.listener(move |this, drag: &AssetDrag, window, cx| {
                        if drag.core == core && drag.from != kind {
                            this.open_transfer_dialog(drag, kind, window, cx);
                        }
                    }))
                    .child(list),
            )
    }

    /// Открыть диалог количества для переноса монеты (дефолт — всё свободное).
    fn open_transfer_dialog(
        &mut self,
        drag: &AssetDrag,
        to: WalletKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_qty = num(drag.free);
        let input = cx.new(|cx| MoonInputState::new(window, cx).default_value(&default_qty));
        self.pending_transfer = Some(PendingTransfer {
            core: drag.core,
            asset: drag.asset.clone(),
            from: drag.from,
            to,
            free: drag.free,
        });
        self.transfer_input = Some(input);
        cx.notify();
    }

    /// Подтвердить перенос: прочитать количество из поля, выполнить и закрыть диалог.
    fn confirm_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(pt) = self.pending_transfer.clone() else {
            return;
        };
        let qty = self
            .transfer_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .and_then(|s| s.trim().replace(',', ".").parse::<f64>().ok())
            .unwrap_or(0.0);
        if qty > 0.0 {
            self.backend.read(cx).session.transfer_asset(
                pt.core,
                pt.asset.clone(),
                qty,
                pt.from,
                pt.to,
            );
        }
        self.close_transfer_dialog(cx);
    }

    fn close_transfer_dialog(&mut self, cx: &mut Context<Self>) {
        self.pending_transfer = None;
        self.transfer_input = None;
        cx.notify();
    }

    /// Оверлей-диалог количества переноса (по центру окна).
    pub(super) fn transfer_dialog(
        &self,
        pt: &PendingTransfer,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let title = format!(
            "Перенос {}: {} → {}",
            pt.asset,
            pt.from.label(),
            pt.to.label()
        );
        let mut card = v_flex()
            .w(px(320.0))
            .gap(design::ui_px(cx, 10.0))
            .p(design::ui_px(cx, 14.0))
            .rounded(px(8.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.border))
            .child(div().text_color(rgb(p.text)).child(title))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child(format!("свободно: {}", num(pt.free))),
            );
        if let Some(input) = &self.transfer_input {
            card = card.child(MoonInput::new("transfer-amount").state(input).small());
        }
        card = card.child(
            h_flex()
                .w_full()
                .gap_2()
                .justify_end()
                .child(
                    MoonButton::new("transfer-cancel")
                        .outline()
                        .size(MoonButtonSize::Action)
                        .label("Отмена")
                        .on_click(cx.listener(|this, _, _, cx| this.close_transfer_dialog(cx)))
                        .render(),
                )
                .child(
                    MoonButton::new("transfer-confirm")
                        .primary()
                        .size(MoonButtonSize::Action)
                        .label("Перенести")
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_transfer(cx)))
                        .render(),
                ),
        );

        // Затемнённый фон на всё окно + карточка по центру.
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .child(card)
    }
}
