use chrono::Local;
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, Message,
    MessageId,
};
use tokio::sync::{mpsc::UnboundedReceiver, Mutex};

use crate::config::{load_saved_state, save_saved_state, Config, SavedState};
use crate::system::{
    cancel_shutdown, capture_primary_screenshot, fetch_system_metrics, lock_workstation,
    media_play_pause, reboot_now, schedule_shutdown, shutdown_now, sleep_pc, volume_down_10,
    volume_mute, volume_up_10,
};

#[derive(Debug)]
pub enum BotCommand {
    SendRemote,
}

pub struct AppState {
    pub config: Config,
    pub sys: Mutex<sysinfo::System>,
    pub shutdown_timer: Mutex<Option<Instant>>,
    pub saved_state: Mutex<SavedState>,
}

/// Сборка главного меню Inline-пульта (сетка по 2-3 кнопки в ряд)
pub fn make_main_keyboard() -> InlineKeyboardMarkup {
    let autostart_enabled = crate::system::is_autostart_enabled();
    let autostart_btn_text = if autostart_enabled {
        "🚀 Автозапуск: ✅ Вкл"
    } else {
        "🚀 Автозапуск: ❌ Выкл"
    };

    let mute_btn_text = if crate::system::is_system_muted().unwrap_or(false) {
        "🔊 Вкл звук"
    } else {
        "🔇 Mute"
    };

    InlineKeyboardMarkup::new(vec![
        // Питание: ряд 1
        vec![
            InlineKeyboardButton::callback("🔌 Выключить", "btn_shutdown"),
            InlineKeyboardButton::callback("💤 Сон", "btn_sleep"),
        ],
        // Питание: ряд 2
        vec![
            InlineKeyboardButton::callback("🔄 Перезагрузка", "btn_reboot"),
            InlineKeyboardButton::callback("🔒 Блокировка", "btn_lock"),
        ],
        // Таймер выключения: ряд 3
        vec![
            InlineKeyboardButton::callback("⏱ 30м", "timer_30m"),
            InlineKeyboardButton::callback("⏱ 1ч", "timer_1h"),
            InlineKeyboardButton::callback("⏱ 2ч", "timer_2h"),
        ],
        // Таймер сброс и обновление статуса: ряд 4
        vec![
            InlineKeyboardButton::callback("❌ Сброс таймера", "timer_reset"),
            InlineKeyboardButton::callback("🔄 Обновить статус", "btn_refresh"),
        ],
        // Медиа и звук: ряд 5
        vec![
            InlineKeyboardButton::callback("🔉 -10%", "vol_down"),
            InlineKeyboardButton::callback(mute_btn_text, "vol_mute"),
            InlineKeyboardButton::callback("🔊 +10%", "vol_up"),
        ],
        // Управление воспроизведением и скриншот: ряд 6
        vec![
            InlineKeyboardButton::callback("⏯ Play/Pause", "media_play_pause"),
            InlineKeyboardButton::callback("📸 Скриншот", "btn_screenshot"),
        ],
        // Автозагрузка Windows: ряд 7
        vec![
            InlineKeyboardButton::callback(autostart_btn_text, "toggle_autostart"),
        ],
    ])
}

/// Меню подтверждения опасных действий: «Точно выполнить? [✅ Да] [❌ Отмена]»
pub fn make_confirm_keyboard(action: &str) -> InlineKeyboardMarkup {
    let confirm_data = match action {
        "shutdown" => "confirm_shutdown",
        "reboot" => "confirm_reboot",
        _ => "cancel_confirm",
    };

    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("✅ Да", confirm_data),
        InlineKeyboardButton::callback("❌ Отмена", "cancel_confirm"),
    ]])
}

/// Клавиатура под отправленным скриншотом для его удаления
pub fn make_photo_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🗑 Удалить",
        "btn_delete_msg",
    )]])
}

/// Формирование текста сообщения с актуальной сводкой системы
pub async fn build_status_text(state: &AppState, prompt_override: Option<&str>) -> String {
    let metrics = {
        let mut sys = state.sys.lock().await;
        fetch_system_metrics(&mut sys)
    };

    let timer_str = {
        let timer = state.shutdown_timer.lock().await;
        match *timer {
            Some(target) => {
                let now = Instant::now();
                if target > now {
                    let total_secs = target.duration_since(now).as_secs();
                    let mins = total_secs / 60;
                    let secs = total_secs % 60;
                    if mins >= 60 {
                        format!("активен (через {} ч {} мин)", mins / 60, mins % 60)
                    } else {
                        format!("активен (через {} мин {:02} сек)", mins, secs)
                    }
                } else {
                    "время истекло".to_string()
                }
            }
            None => "не активен".to_string(),
        }
    };

    let autostart_str = if crate::system::is_autostart_enabled() {
        "Включена"
    } else {
        "Отключена"
    };

    let audio_str = match (crate::system::get_system_volume(), crate::system::is_system_muted()) {
        (Some(vol), Some(true)) => format!("{}% (🔇 Выключен)", vol),
        (Some(vol), Some(false)) => format!("{}%", vol),
        (Some(vol), None) => format!("{}%", vol),
        _ => "недоступно".to_string(),
    };

    let current_time = Local::now().format("%H:%M:%S").to_string();

    let mut text = format!(
        "🟢 ПК запущен и на связи\n\n\
        📊 Состояние системы:\n\
        • Статус: Онлайн\n\
        • Загрузка CPU: {:.1}%\n\
        • Занято RAM: {:.2} ГБ / {:.2} ГБ ({}%)\n\
        • Громкость: {}\n\
        • Таймер выключения: {}\n\
        • Автозагрузка Windows: {}\n\
        🕒 Время обновления: {}",
        metrics.cpu_usage,
        metrics.used_ram_gb,
        metrics.total_ram_gb,
        metrics.ram_percent,
        audio_str,
        timer_str,
        autostart_str,
        current_time
    );

    if let Some(extra) = prompt_override {
        text.push_str("\n\n");
        text.push_str(extra);
    }

    text
}

/// Отправка нового сообщения с пультом управления с автоматическим удалением старого
pub async fn send_remote_panel(bot: &Bot, chat_id: ChatId, state: &AppState) {
    // 1. Удаляем предыдущий пульт из чата, чтобы они не скапливались
    let old_panel_id = {
        let mut saved = state.saved_state.lock().await;
        saved.last_panel_msg_id.take()
    };
    if let Some(msg_id) = old_panel_id {
        let _ = bot.delete_message(chat_id, MessageId(msg_id)).await;
    }

    // 2. Отправляем актуальный пульт и сохраняем его ID
    let text = build_status_text(state, None).await;
    let keyboard = make_main_keyboard();
    if let Ok(sent_msg) = bot.send_message(chat_id, text).reply_markup(keyboard).await {
        let new_id = sent_msg.id.0;
        let mut saved = state.saved_state.lock().await;
        saved.last_panel_msg_id = Some(new_id);
        save_saved_state(&saved);
    }
}

/// Обработчик входящих текстовых сообщений
async fn handle_message(bot: Bot, msg: Message, state: Arc<AppState>) -> ResponseResult<()> {
    let from_id = match msg.from.as_ref() {
        Some(user) => user.id.0,
        None => return Ok(()),
    };

    // Строгая проверка ID отправителя: чужие запросы полностью игнорируются
    if from_id != state.config.allowed_user_id {
        return Ok(());
    }

    let chat_id = msg.chat.id;
    let curr_id = msg.id.0;

    // Проверяем команду очистки или старта: /clean, /clear, /start
    let is_clean_cmd = msg
        .text()
        .map_or(false, |t| t == "/clean" || t == "/clear" || t == "/start");

    if is_clean_cmd {
        // Очищаем накопившийся мусор (диапазон последних сообщений)
        for id in (curr_id - 35)..=curr_id {
            let _ = bot.delete_message(chat_id, MessageId(id)).await;
        }
    } else {
        // Всегда удаляем входящее сообщение пользователя, чтобы чат оставался абсолютно чистым
        let _ = bot.delete_message(chat_id, msg.id).await;
    }

    // Отправляем актуальный единый пульт
    send_remote_panel(&bot, chat_id, &state).await;
    Ok(())
}

/// Обработчик нажатий на Inline-кнопки (CallbackQuery)
async fn handle_callback_query(
    bot: Bot,
    q: CallbackQuery,
    state: Arc<AppState>,
) -> ResponseResult<()> {
    // Строгая проверка ID нажавшего: любые запросы от чужих ID полностью игнорируются
    if q.from.id.0 != state.config.allowed_user_id {
        return Ok(());
    }

    let data = match q.data.as_deref() {
        Some(d) => d,
        None => return Ok(()),
    };

    let message = match &q.message {
        Some(m) => m,
        None => return Ok(()),
    };

    let chat_id = message.chat().id;
    let msg_id = message.id();

    match data {
        // --- Обновление статуса ---
        "btn_refresh" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🔄 Статус обновлен")
                .await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        // --- Запрос на выключение ПК (с подтверждением) ---
        "btn_shutdown" => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            let text = build_status_text(
                &state,
                Some("⚠️ Выключение ПК: Точно выполнить?"),
            )
            .await;
            let keyboard = make_confirm_keyboard("shutdown");
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        // --- Запрос на перезагрузку ПК (с подтверждением) ---
        "btn_reboot" => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            let text = build_status_text(
                &state,
                Some("⚠️ Перезагрузка ПК: Точно выполнить?"),
            )
            .await;
            let keyboard = make_confirm_keyboard("reboot");
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        // --- Отмена подтверждения (возврат в главное меню) ---
        "cancel_confirm" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("❌ Действие отменено")
                .await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        // --- Подтвержденное выключение ---
        "confirm_shutdown" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🔌 Компьютер выключается...")
                .await;
            let _ = bot
                .edit_message_text(chat_id, msg_id, "🔌 Компьютер выключается...")
                .await;
            shutdown_now();
        }

        // --- Подтвержденная перезагрузка ---
        "confirm_reboot" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🔄 Компьютер перезагружается...")
                .await;
            let _ = bot
                .edit_message_text(chat_id, msg_id, "🔄 Компьютер перезагружается...")
                .await;
            reboot_now();
        }

        // --- Переход в спящий режим ---
        "btn_sleep" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("💤 Переход в спящий режим...")
                .await;
            let text =
                build_status_text(&state, Some("💤 Компьютер переходит в спящий режим...")).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
            sleep_pc();
        }

        // --- Блокировка Windows ---
        "btn_lock" => {
            lock_workstation();
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🔒 ПК заблокирован")
                .await;
            let text = build_status_text(&state, Some("🔒 Рабочая станция заблокирована.")).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        // --- Таймеры выключения ---
        "timer_30m" => {
            schedule_shutdown(1800);
            {
                let mut timer = state.shutdown_timer.lock().await;
                *timer = Some(Instant::now() + std::time::Duration::from_secs(1800));
            }
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("⏱ Таймер выключения установлен на 30 минут")
                .await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        "timer_1h" => {
            schedule_shutdown(3600);
            {
                let mut timer = state.shutdown_timer.lock().await;
                *timer = Some(Instant::now() + std::time::Duration::from_secs(3600));
            }
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("⏱ Таймер выключения установлен на 1 час")
                .await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        "timer_2h" => {
            schedule_shutdown(7200);
            {
                let mut timer = state.shutdown_timer.lock().await;
                *timer = Some(Instant::now() + std::time::Duration::from_secs(7200));
            }
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("⏱ Таймер выключения установлен на 2 часа")
                .await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        "timer_reset" => {
            cancel_shutdown();
            {
                let mut timer = state.shutdown_timer.lock().await;
                *timer = None;
            }
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("❌ Таймер выключения сброшен")
                .await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        // --- Медиа и громкость ---
        "vol_down" => {
            let vol = volume_down_10();
            let msg = if vol > 0 {
                format!("🔉 Громкость: {}%", vol)
            } else {
                "🔉 Громкость: 0% (минимум)".to_string()
            };
            let _ = bot.answer_callback_query(q.id.clone()).text(msg).await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        "vol_mute" => {
            let is_muted = volume_mute();
            let msg = if is_muted {
                "🔇 Звук выключен (Mute)"
            } else {
                "🔊 Звук включен"
            };
            let _ = bot.answer_callback_query(q.id.clone()).text(msg).await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        "vol_up" => {
            let vol = volume_up_10();
            let msg = if vol > 0 {
                format!("🔊 Громкость: {}%", vol)
            } else {
                "🔊 Громкость: +10%".to_string()
            };
            let _ = bot.answer_callback_query(q.id.clone()).text(msg).await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        "media_play_pause" => {
            media_play_pause();
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("⏯ Play/Pause")
                .await;
        }

        // --- Захват экрана (скриншот) ---
        "btn_screenshot" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("📸 Делаю снимок экрана...")
                .await;

            // Удаляем предыдущий скриншот из чата, если он еще не был удален
            let old_photo = {
                let mut saved = state.saved_state.lock().await;
                saved.last_photo_msg_id.take()
            };
            if let Some(photo_id) = old_photo {
                let _ = bot.delete_message(chat_id, MessageId(photo_id)).await;
            }

            match tokio::task::spawn_blocking(capture_primary_screenshot).await {
                Ok(Ok(png_bytes)) => {
                    let time_str = Local::now().format("%H:%M:%S").to_string();
                    let photo = InputFile::memory(png_bytes).file_name("screenshot.png");
                    let delete_kb = make_photo_keyboard();
                    if let Ok(sent_photo) = bot
                        .send_photo(chat_id, photo)
                        .caption(format!("📸 Скриншот экрана [{}]", time_str))
                        .reply_markup(delete_kb)
                        .await
                    {
                        let mut saved = state.saved_state.lock().await;
                        saved.last_photo_msg_id = Some(sent_photo.id.0);
                        save_saved_state(&saved);
                    }
                }
                Ok(Err(e)) => {
                    let _ = bot
                        .send_message(chat_id, format!("❌ Ошибка захвата экрана: {}", e))
                        .await;
                }
                Err(e) => {
                    let _ = bot
                        .send_message(
                            chat_id,
                            format!("❌ Ошибка выполнения задачи захвата: {}", e),
                        )
                        .await;
                }
            }
        }

        // --- Удаление сообщения (скриншота) ---
        "btn_delete_msg" => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🗑 Снимок удален")
                .await;
            let _ = bot.delete_message(chat_id, msg_id).await;
            let mut saved = state.saved_state.lock().await;
            if saved.last_photo_msg_id == Some(msg_id.0) {
                saved.last_photo_msg_id = None;
                save_saved_state(&saved);
            }
        }

        // --- Переключение автозагрузки Windows ---
        "toggle_autostart" => {
            let current = crate::system::is_autostart_enabled();
            let new_state = !current;
            let _ = crate::system::set_autostart(new_state);

            let mut updated_config = state.config.clone();
            updated_config.autostart = new_state;
            let _ = crate::config::save_config(&updated_config);

            let alert = if new_state {
                "🚀 Автозагрузка Windows ВКЛЮЧЕНА!"
            } else {
                "🚀 Автозагрузка Windows ОТКЛЮЧЕНА!"
            };

            let _ = bot.answer_callback_query(q.id.clone()).text(alert).await;
            let text = build_status_text(&state, None).await;
            let keyboard = make_main_keyboard();
            let _ = bot
                .edit_message_text(chat_id, msg_id, text)
                .reply_markup(keyboard)
                .await;
        }

        _ => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
        }
    }

    Ok(())
}

/// Точка входа в фоновый асинхронный сервис бота
pub async fn run_bot(config: Config, mut cmd_rx: UnboundedReceiver<BotCommand>) {
    let bot = Bot::new(&config.bot_token);
    let owner_chat_id = ChatId(config.allowed_user_id as i64);

    let initial_saved = load_saved_state();
    let state = Arc::new(AppState {
        config: config.clone(),
        sys: Mutex::new(sysinfo::System::new_all()),
        shutdown_timer: Mutex::new(None),
        saved_state: Mutex::new(initial_saved),
    });

    // При старте программы бот автоматически отправляет владельцу приветствие и пульт
    send_remote_panel(&bot, owner_chat_id, &state).await;

    // Фоновая задача для обработки команд из трей-меню (например, «Отправить пульт в TG»)
    let bot_for_cmds = bot.clone();
    let state_for_cmds = state.clone();
    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                BotCommand::SendRemote => {
                    send_remote_panel(&bot_for_cmds, owner_chat_id, &state_for_cmds).await;
                }
            }
        }
    });

    // Настройка диспетчера и Long Polling teloxide
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_message))
        .branch(Update::filter_callback_query().endpoint(handle_callback_query));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .build()
        .dispatch()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_keyboard_structure() {
        let kb = make_main_keyboard();
        // 7 рядов кнопок
        assert_eq!(kb.inline_keyboard.len(), 7);

        // Ряд 1: Выключить, Сон
        assert_eq!(kb.inline_keyboard[0].len(), 2);
        assert_eq!(kb.inline_keyboard[0][0].text, "🔌 Выключить");
        assert_eq!(kb.inline_keyboard[0][1].text, "💤 Сон");

        // Ряд 2: Перезагрузка, Блокировка
        assert_eq!(kb.inline_keyboard[1].len(), 2);
        assert_eq!(kb.inline_keyboard[1][0].text, "🔄 Перезагрузка");
        assert_eq!(kb.inline_keyboard[1][1].text, "🔒 Блокировка");

        // Ряд 3: Таймеры 30м, 1ч, 2ч
        assert_eq!(kb.inline_keyboard[2].len(), 3);
        assert_eq!(kb.inline_keyboard[2][0].text, "⏱ 30м");
        assert_eq!(kb.inline_keyboard[2][1].text, "⏱ 1ч");
        assert_eq!(kb.inline_keyboard[2][2].text, "⏱ 2ч");

        // Ряд 4: Сброс таймера, Обновить статус
        assert_eq!(kb.inline_keyboard[3].len(), 2);
        assert_eq!(kb.inline_keyboard[3][0].text, "❌ Сброс таймера");
        assert_eq!(kb.inline_keyboard[3][1].text, "🔄 Обновить статус");

        // Ряд 5: -10%, Mute, +10%
        assert_eq!(kb.inline_keyboard[4].len(), 3);
        assert_eq!(kb.inline_keyboard[4][0].text, "🔉 -10%");
        assert!(
            kb.inline_keyboard[4][1].text == "🔇 Mute"
                || kb.inline_keyboard[4][1].text == "🔊 Вкл звук"
        );
        assert_eq!(kb.inline_keyboard[4][2].text, "🔊 +10%");

        // Ряд 6: Play/Pause, Скриншот
        assert_eq!(kb.inline_keyboard[5].len(), 2);
        assert_eq!(kb.inline_keyboard[5][0].text, "⏯ Play/Pause");
        assert_eq!(kb.inline_keyboard[5][1].text, "📸 Скриншот");

        // Ряд 7: Автозапуск Windows
        assert_eq!(kb.inline_keyboard[6].len(), 1);
        assert!(kb.inline_keyboard[6][0].text.starts_with("🚀 Автозапуск:"));
    }

    #[test]
    fn test_confirm_keyboard_callbacks() {
        let kb_shutdown = make_confirm_keyboard("shutdown");
        assert_eq!(kb_shutdown.inline_keyboard.len(), 1);
        assert_eq!(kb_shutdown.inline_keyboard[0][0].text, "✅ Да");
        assert_eq!(kb_shutdown.inline_keyboard[0][1].text, "❌ Отмена");

        let kb_reboot = make_confirm_keyboard("reboot");
        assert_eq!(kb_reboot.inline_keyboard.len(), 1);
        assert_eq!(kb_reboot.inline_keyboard[0][0].text, "✅ Да");
        assert_eq!(kb_reboot.inline_keyboard[0][1].text, "❌ Отмена");
    }

    #[test]
    fn test_photo_keyboard() {
        let kb = make_photo_keyboard();
        assert_eq!(kb.inline_keyboard.len(), 1);
        assert_eq!(kb.inline_keyboard[0][0].text, "🗑 Удалить");
    }
}

