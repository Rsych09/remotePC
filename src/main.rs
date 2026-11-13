#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bot;
mod config;
mod system;

use bot::BotCommand;
use config::{load_or_create_config, show_native_message_box};
use system::{is_autostart_enabled, set_autostart};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::mpsc;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// Программная генерация аккуратной зеленой круглой иконки 32x32 в оперативной памяти
fn create_green_tray_icon() -> Icon {
    const WIDTH: u32 = 32;
    const HEIGHT: u32 = 32;
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= 12.0 {
                // Приятный изумрудно-зеленый цвет
                rgba.extend_from_slice(&[34, 197, 94, 255]);
            } else if dist <= 14.0 {
                // Сглаживание границы круга
                let alpha = ((14.0 - dist) / 2.0 * 255.0).clamp(0.0, 255.0) as u8;
                rgba.extend_from_slice(&[34, 197, 94, alpha]);
            } else {
                // Полностью прозрачный фон
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Icon::from_rgba(rgba, WIDTH, HEIGHT).expect("Не удалось создать иконку системного трея")
}

fn main() {
    // 1. Загрузка конфигурации или создание шаблона при первом запуске
    let mut config = match load_or_create_config() {
        Ok(cfg) => cfg,
        Err(err_msg) => {
            show_native_message_box("Remote PC Agent — Настройка", &err_msg);
            return;
        }
    };

    // 2. Если в конфигурации активен автозапуск — автоматически регистрируем программу в реестре Windows
    if config.autostart && !is_autostart_enabled() {
        let _ = set_autostart(true);
    } else if !config.autostart && is_autostart_enabled() {
        let _ = set_autostart(false);
    }

    // Канал связи из потока UI трея в асинхронный рантайм бота
    let (bot_cmd_tx, bot_cmd_rx) = mpsc::unbounded_channel::<BotCommand>();

    // 3. В отдельном потоке запускаем рантайм Tokio и цикл Long Polling для teloxide
    let bot_config = config.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Ошибка инициализации асинхронного рантайма Tokio");

        rt.block_on(async {
            bot::run_bot(bot_config, bot_cmd_rx).await;
        });
    });

    // 4. В главном потоке инициализируем системный трей и нативный цикл событий Windows (tao)
    let event_loop = EventLoopBuilder::new().build();

    let tray_menu = Menu::new();
    let item_send = MenuItem::new("Отправить пульт в TG", true, None);
    let autostart_state = is_autostart_enabled();
    let item_autostart = CheckMenuItem::new(
        "Автозагрузка Windows (Вкл/Выкл)",
        true,
        autostart_state,
        None,
    );
    let item_exit = MenuItem::new("Выход", true, None);

    let _ = tray_menu.append(&item_send);
    let _ = tray_menu.append(&item_autostart);
    let _ = tray_menu.append(&item_exit);

    let tray_icon = create_green_tray_icon();
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_icon(tray_icon)
        .with_tooltip("Remote PC Agent (Telegram Control)")
        .build()
        .expect("Не удалось инициализировать иконку в системном трее Windows");

    let menu_channel = MenuEvent::receiver();

    // 5. Нативный цикл обработки сообщений Windows
    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(50),
        );

        while let Ok(event) = menu_channel.try_recv() {
            if event.id == item_send.id() {
                let _ = bot_cmd_tx.send(BotCommand::SendRemote);
            } else if event.id == item_autostart.id() {
                let new_state = !is_autostart_enabled();
                let _ = set_autostart(new_state);
                item_autostart.set_checked(new_state);
                config.autostart = new_state;
                let _ = crate::config::save_config(&config);
            } else if event.id == item_exit.id() {
                *control_flow = ControlFlow::Exit;
                return;
            }
        }
    });
}
