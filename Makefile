# MoonTerminal — сборка единственного бинаря `moonterminal` (crates/moon-ui-gpui).
#
#   make run            собрать и запустить (debug)
#   make build          собрать (debug)
#   make release        собрать (release)
#   make check          быстрая проверка типов
#   make fmt            cargo fmt
#   make clean          очистить target
#   make update-forks   обновить ZedFork/MoonPalette до HEAD веток + перелочить Cargo.lock
#
# Windows: таргет ВСЕГДА MSVC (x86_64-pc-windows-msvc), не GNU — его ожидают GPUI/DirectX/
# chartdx. Запускать `make` из «x64 Native Tools Command Prompt for VS 2022» (там настроен
# vcvars), иначе линковка C-зависимостей не найдёт link.exe.
# macOS (Metal) / Linux: нативный таргет, отдельная настройка не нужна.

PKG := -p moon-ui-gpui --bin moonterminal

ifeq ($(OS),Windows_NT)
  TARGET := --target x86_64-pc-windows-msvc
  BIN := target\x86_64-pc-windows-msvc\debug\moonterminal.exe
else
  TARGET :=
  BIN := target/debug/moonterminal
endif

.PHONY: run build release check fmt clean update-forks help

help:
	@echo "make run | build | release | check | fmt | clean | update-forks"
	@echo "bin: $(BIN)"

run:
	cargo run $(PKG) $(TARGET)

build:
	cargo build $(PKG) $(TARGET)

release:
	cargo build --release $(PKG) $(TARGET)

check:
	cargo check $(PKG) $(TARGET)

fmt:
	cargo fmt

clean:
	cargo clean

# Форки ZedFork/MoonPalette пинятся по веткам (master/main); Cargo.lock закоммичен и фиксирует
# точные rev. Обновление — ОСОЗНАННОЕ: подтянуть HEAD веток, проверить сборку, закоммитить lock.
update-forks:
	cargo update
	@echo ">> Форки обновлены. Теперь: make build  →  git add Cargo.lock && git commit"
