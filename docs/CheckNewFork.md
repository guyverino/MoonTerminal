# CheckNewFork

Чеклист для перехода на новый чистый GPUI/Zed fork. Старый форк может быть донором идей, но
каждый пункт ниже надо заново проверить в новом дереве, а не считать автоматически перенесённым.

## Windows multi-monitor restore: DPI round-trip

Статус в старом форке: найден и исправлен локально 2026-06-16.

### Симптом

Откреплённое окно графика восстанавливается на правильном мониторе через `WindowOptions.display_id`,
но на мониторе с DPI scale != primary его `origin` сдвигается на коэффициент масштаба монитора.

Реальный пример с верхним монитором `scale=1.25`:

```text
saved window_bounds():    x=95  y=-733  w=896  h=611
restored window_bounds(): x=119 y=-916  w=892  h=602
```

`119/95` и `916/733` примерно равны `1.25`, то есть logical coordinates были round-trip-нуты через
не тот scale-factor.

### Причина

В `gpui_windows/src/window.rs::retrieve_window_placement` saved logical bounds переводились в
device pixels через `window.state.scale_factor` свежесозданного HWND. При создании окна сразу на
не-primary monitor этот HWND scale может ещё отражать primary/current monitor, а не `display_id`,
который мы явно передали в `WindowOptions`.

Итог: `WindowBounds::Windowed(bounds) + display_id` не round-trip-ится с `window.window_bounds()`.

### Фикс, который надо перенести в новый fork

В `gpui_windows/src/display.rs` дать `WindowsDisplay` внутренний getter:

```rust
pub(crate) fn scale_factor(&self) -> f32 {
    self.scale_factor
}
```

В `gpui_windows/src/window.rs`, перед `SetWindowPlacement`, брать scale именно у выбранного
`display`:

```rust
let display_scale_factor = display.scale_factor();
this.state.scale_factor.set(display_scale_factor);
this.state
    .direct_manipulation
    .set_scale_factor(display_scale_factor);

let placement = retrieve_window_placement(
    hwnd,
    display,
    params.bounds,
    display_scale_factor,
    &this.state.border_offset,
)?;
```

И внутри `retrieve_window_placement` параметр должен быть именно `display_scale_factor`, а не
случайный initial HWND scale:

```rust
let bounds = bounds.to_device_pixels(display_scale_factor);
```

Комментарий для PR: initial HWND DPI может ещё быть DPI primary/current monitor, а saved bounds
лежат в logical coordinate space целевого display, поэтому placement обязан использовать DPI scale
этого `display_id`.

### Как проверить руками

1. Windows, два монитора с разным scale, например primary `100%`, secondary `125%`.
2. Открыть detached chart window, перенести его на secondary monitor, закрыть терминал.
3. Записать сохранённые `window.window_bounds()` из `charts.json` или диагностического лога.
4. Запустить терминал снова и сразу после restore записать `window.window_bounds()`.
5. Ожидание: `origin` round-trip-ится в те же logical coordinates, без умножения на `1.25`.
   Допустима мелкая разница размера из-за border/frame, но не scale-сдвиг по `x/y`.
6. Повторить close/restart 2-3 раза: окно не должно дрейфовать.
7. Повторить на primary monitor: обычный restore не должен сломаться.

Если после restore `x/y` стали примерно `saved * monitor_scale`, фикс не перенесён или scale снова
берётся от HWND не того монитора.
