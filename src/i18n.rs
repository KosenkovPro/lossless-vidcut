//! Translations of the user interface.
//!
//! The application speaks English and Russian. A string is identified by its
//! English text, which keeps the call sites readable and makes a missing
//! translation harmless: an unknown key simply falls back to English.
//!
//! ```
//! use lossless_vidcut::i18n::{tr, Lang};
//! assert_eq!(tr(Lang::English, "Settings"), "Settings");
//! assert_eq!(tr(Lang::Russian, "Settings"), "Настройки");
//! ```

use serde::{Deserialize, Serialize};

/// The languages the user interface is available in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    /// English, the original language of the application.
    #[default]
    English,
    /// Russian.
    Russian,
}

impl Lang {
    /// The name of the language in the language itself.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Russian => "Русский",
        }
    }

    /// Every language the user can choose from.
    pub fn all() -> [Lang; 2] {
        [Lang::English, Lang::Russian]
    }

    /// Detect the language of the operating system.
    ///
    /// This is only used for the very first start, when no settings file
    /// exists yet. Afterwards the user decides.
    pub fn detect() -> Self {
        if system_uses_russian() {
            Lang::Russian
        } else {
            Lang::English
        }
    }
}

/// Translate `key` into `lang`.
///
/// The English text is the key, so the English language needs no table at all.
pub fn tr(lang: Lang, key: &'static str) -> &'static str {
    match lang {
        Lang::English => key,
        Lang::Russian => ru(key),
    }
}

/// Translate `key` into `lang` and fill in the named placeholders.
///
/// The placeholders are written as `{name}` and are replaced one after the
/// other, so a translated message has to use exactly the same names as the
/// English one.
pub fn tf(lang: Lang, key: &'static str, args: &[(&str, &str)]) -> String {
    let template = tr(lang, key);
    let mut text = template.to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The keyboard shortcuts that are shown in the help window.
///
/// Every description has to exist in the Russian table, the test below makes
/// sure of that.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "Play / pause"),
    ("← / →", "Step one second"),
    ("Shift + ← / →", "Step ten seconds"),
    (", / .", "Previous / next frame"),
    ("I", "Set the in point"),
    ("O", "Set the out point"),
    ("Ctrl + Shift + I / O", "Set the in and out point"),
    ("S / B / Ctrl + B", "Split at the play head"),
    ("Enter", "Cut the marked range into the timeline"),
    ("Home / End", "Jump to the start / the end"),
    ("Delete", "Delete the selected clip"),
    ("Ctrl + D", "Duplicate the selected clip"),
    ("Ctrl + A", "Select all clips"),
    ("Ctrl + Z", "Undo"),
    ("Ctrl + Shift + Z", "Redo"),
    ("+ / -", "Zoom in / out"),
    ("Ctrl + 0", "Fit the timeline into the window"),
    ("Ctrl + O", "Open a file"),
    ("Ctrl + E", "Export the timeline"),
    ("Ctrl + ,", "Open the settings"),
    ("Ctrl + /", "Show the list of shortcuts"),
];

/// The Russian translations of the English texts of the interface.
///
/// The English text is the key, so that a missing translation falls back to
/// English instead of showing an empty label.
const TRANSLATIONS: &[(&str, &str)] = &[
    ("Open file…", "Открыть файл…"),
    ("Add media…", "Добавить медиа…"),
    ("Settings", "Настройки"),
    ("Export", "Экспорт"),
    ("Cut", "Резать"),
    ("Undo", "Отменить"),
    ("Redo", "Повторить"),
    ("Delete", "Удалить"),
    ("Duplicate", "Дублировать"),
    ("Language", "Язык"),
    ("Keyboard shortcuts", "Горячие клавиши"),
    ("Ready", "Готово"),
    ("Cancel", "Отменить"),
    ("cancelled", "отменено"),
    ("Close", "Закрыть"),
    ("Save", "Сохранить"),
    ("Choose…", "Выбрать…"),
    ("Fit", "Вписать"),
    ("Retry", "Повторить"),
    ("Media", "Медиа"),
    ("Add to timeline", "Добавить на таймлайн"),
    ("No media yet", "Медиа пока нет"),
    ("Drag video files here", "Перетащите видеофайлы сюда"),
    ("Reading…", "Чтение…"),
    ("Loaded {name}", "Загружено: {name}"),
    ("Could not read the file: {error}", "Не удалось прочитать файл: {error}"),
    ("Play", "Воспроизвести"),
    ("Pause", "Пауза"),
    ("Speed", "Скорость"),
    ("Reset", "Сброс"),
    ("Start", "Начало"),
    ("End", "Конец"),
    ("Source", "Источник"),
    ("In", "Начало"),
    ("Out", "Конец"),
    ("Duration", "Длительность"),
    ("Key frames", "Ключевые кадры"),
    ("Snapping to key frames", "Привязка к ключевым кадрам"),
    ("Nothing is selected", "Ничего не выбрано"),
    ("Timeline", "Таймлайн"),
    ("The timeline is empty", "Таймлайн пуст"),
    ("Drop videos here, cut them with the scissors and drag the pieces into order.", "Перетащите сюда видео, нарежьте ножницами и переставьте куски в нужном порядке."),
    ("Total {time}", "Итого {time}"),
    ("{count} clip(s)", "Клипов: {count}"),
    ("Split at the play head", "Разрезать по позиции воспроизведения"),
    ("The clip was added to the timeline.", "Клип добавлен на таймлайн."),
    ("The clip was removed.", "Клип удалён."),
    ("The clip was duplicated.", "Клип продублирован."),
    ("Nothing to cut — mark an in and an out point first.", "Нечего резать — сначала отметьте начало и конец."),
    ("There is nothing to cut here.", "Здесь нечего резать."),
    ("The clip was cut at {time}.", "Клип разрезан в позиции {time}."),
    ("The clip was cut at the closest key frame ({time}).", "Клип разрезан по ближайшему ключевому кадру ({time})."),
    ("There is nothing to export.", "Нечего экспортировать."),
    ("The result must not be written over a file of the project.", "Результат нельзя записывать поверх файла проекта."),
    ("{count} clip(s) were skipped because their file is gone.", "Пропущено клипов: {count} — их файлы больше не доступны."),
    ("Nothing to undo.", "Отменять нечего."),
    ("Nothing to redo.", "Повторять нечего."),
    ("Clip", "Клип"),
    ("No clip is selected", "Клип не выбран"),
    ("Encoding", "Кодирование"),
    ("Lossless (copy)", "Без потерь (копирование)"),
    ("Precise (re-encode)", "Точно (перекодирование)"),
    ("Output file", "Итоговый файл"),
    ("not chosen", "не выбрано"),
    ("Add at least one clip before exporting.", "Добавьте хотя бы один клип перед экспортом."),
    ("Choose where the result should be written.", "Выберите, куда сохранить результат."),
    ("Export the timeline", "Экспортировать таймлайн"),
    ("Snap the markers to key frames", "Привязывать метки к ключевым кадрам"),
    ("The files differ: codecs [{codecs}], resolutions [{resolutions}], frame rates [{rates}], audio [{audio}]. A stream copy may fail, use “Precise” to re-encode.", "Файлы различаются: кодекы [{codecs}], разрешения [{resolutions}], частоты кадров [{rates}], звук [{audio}]. Копирование потока может не сработать — используйте «Точно»."),
    ("All clips share the same format.", "Все клипы в одном формате."),
    ("FFmpeg location", "Расположение FFmpeg"),
    ("searched in PATH", "ищется в PATH"),
    ("Auto detect", "Определить автоматически"),
    ("Seek step", "Шаг перемотки"),
    ("Fast seek step", "Быстрый шаг перемотки"),
    ("Dark theme", "Тёмная тема"),
    ("Snap to key frames by default", "Привязка к ключевым кадрам по умолчанию"),
    ("Desktop shortcut", "Ярлык на рабочем столе"),
    ("Create desktop shortcut", "Создать ярлык на рабочем столе"),
    ("Remove desktop shortcut", "Удалить ярлык с рабочего стола"),
    ("Press “Save” to store the settings in the configuration folder.", "Нажмите «Сохранить», чтобы записать настройки в папку конфигурации."),
    ("Settings saved.", "Настройки сохранены."),
    ("Could not save: {error}", "Не удалось сохранить: {error}"),
    ("The desktop shortcut was created.", "Ярлык на рабочем столе создан."),
    ("The desktop shortcut was removed.", "Ярлык с рабочего стола удалён."),
    ("Could not create the desktop shortcut: {error}", "Не удалось создать ярлык: {error}"),
    ("Could not remove the desktop shortcut: {error}", "Не удалось удалить ярлык: {error}"),
    ("Reading the media file…", "Чтение медиафайла…"),
    ("FFmpeg was not found.", "FFmpeg не найден."),
    ("Install it or point the application at the folder that contains ffmpeg and ffprobe.", "Установите его или укажите папку, в которой лежат ffmpeg и ffprobe."),
    ("Locate FFmpeg…", "Найти FFmpeg…"),
    ("FFmpeg found.", "FFmpeg найден."),
    ("The folder does not contain ffmpeg and ffprobe.", "В папке нет ffmpeg и ffprobe."),
    ("The FFmpeg tools are not available.", "Инструменты FFmpeg недоступны."),
    ("In point set to {time}", "Начало отмечено: {time}"),
    ("Out point set to {time}", "Конец отмечен: {time}"),
    ("Started exporting {count} clip(s).", "Начат экспорт клипов: {count}."),
    ("Exporting {count} clip(s)", "Экспорт клипов: {count}"),
    ("{count} running", "Выполняется: {count}"),
    ("Open a video file to get started", "Откройте видеофайл, чтобы начать"),
    ("Drag and drop a file onto this window, or use “Open file”.", "Перетащите файл в это окно или нажмите «Открыть файл»."),
    ("Play / pause", "Воспроизведение / пауза"),
    ("Step one second", "Шаг в одну секунду"),
    ("Step ten seconds", "Шаг в десять секунд"),
    ("Previous / next frame", "Предыдущий / следующий кадр"),
    ("Set the in point", "Отметить начало"),
    ("Set the out point", "Отметить конец"),
    ("Set the in and out point", "Отметить начало и конец"),
    ("Cut the marked range into the timeline", "Вырезать отмеченный участок на таймлайн"),
    ("Jump to the start / the end", "Переход в начало / конец"),
    ("Delete the selected clip", "Удалить выбранный клип"),
    ("Duplicate the selected clip", "Дублировать выбранный клип"),
    ("Select all clips", "Выделить все клипы"),
    ("Zoom in / out", "Приблизить / отдалить"),
    ("Fit the timeline into the window", "Вписать таймлайн в окно"),
    ("Open the settings", "Открыть настройки"),
    ("Open a file", "Открыть файл"),
    ("Show the list of shortcuts", "Показать список горячих клавиш"),
];

/// The Russian translation of an English string.
fn ru(key: &'static str) -> &'static str {
    TRANSLATIONS
        .iter()
        .find(|(english, _)| *english == key)
        .map_or(key, |(_, russian)| *russian)
}

/// `true` when the operating system is configured for Russian.
#[cfg(windows)]
fn system_uses_russian() -> bool {
    // The language identifier of the user interface, as returned by Windows.
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: the function takes no arguments and only reads a global of the
    // current process.
    let language = unsafe { GetUserDefaultUILanguage() };
    // The primary language is the lower ten bits. Russian is 0x19.
    language & 0x03ff == 0x0019
}

/// `true` when the operating system is configured for Russian.
#[cfg(not(windows))]
fn system_uses_russian() -> bool {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(name) else {
            continue;
        };
        let value = value.to_lowercase();
        if value == "ru" || value.starts_with("ru_") || value.starts_with("ru.") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strings that are deliberately not translated.
    const UNTRANSLATED: &[&str] = &["English", "Русский"];

    /// The sources that build the user interface.
    const SOURCES: &[&str] = &[
        include_str!("app.rs"),
        include_str!("editor.rs"),
        include_str!("sequence.rs"),
        include_str!("settings.rs"),
        include_str!("shortcut.rs"),
        include_str!("timeline.rs"),
        include_str!("toast.rs"),
        include_str!("jobs.rs"),
        include_str!("loader.rs"),
        include_str!("player.rs"),
        include_str!("util.rs"),
        include_str!("ffmpeg_tools.rs"),
        include_str!("probe.rs"),
        include_str!("processor.rs"),
        include_str!("project.rs"),
    ];

    /// Find the next call of the function `name` in `source`.
    ///
    /// The name has to start a new word, so that `as_str()` is not mistaken
    /// for a call of `tr()`.
    fn find_call(source: &str, name: &str) -> Option<usize> {
        let mut offset = 0usize;
        while let Some(position) = source[offset..].find(name) {
            let absolute = offset + position;
            let starts_a_word = source[..absolute]
                .chars()
                .next_back()
                .map(|character| !character.is_alphanumeric() && character != '_')
                .unwrap_or(true);
            if starts_a_word {
                return Some(absolute);
            }
            offset = absolute + 1;
        }
        None
    }

    /// Collect the string literals that are passed to `tr` or `tf`.
    fn used_keys(source: &str) -> Vec<&'static str> {
        let mut keys = Vec::new();
        let bytes = source.as_bytes();
        let mut index = 0usize;
        while index < bytes.len() {
            let rest = &source[index..];
            // The next call of either function, whichever comes first.
            let found = match (find_call(rest, "tr("), find_call(rest, "tf(")) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            let Some(found) = found else {
                break;
            };
            index += found + 3;
            // Skip the language argument.
            let tail = &source[index..];
            let Some(comma) = tail.find(',') else {
                continue;
            };
            let tail = &tail[comma + 1..];
            let trimmed = tail.trim_start();
            let offset = tail.len() - trimmed.len();
            if !trimmed.starts_with('"') {
                continue;
            }
            let literal = &trimmed[1..];
            let Some(end) = literal.find('"') else {
                continue;
            };
            let key: &'static str = Box::leak(literal[..end].to_string().into_boxed_str());
            keys.push(key);
            index += comma + 1 + offset + 1 + end;
        }
        keys
    }

    #[test]
    fn english_is_the_identity() {
        assert_eq!(tr(Lang::English, "Settings"), "Settings");
        assert_eq!(tr(Lang::English, "Anything at all"), "Anything at all");
    }

    #[test]
    fn russian_is_used_for_known_keys() {
        assert_eq!(tr(Lang::Russian, "Settings"), "Настройки");
        assert_eq!(tr(Lang::Russian, "Export"), "Экспорт");
        assert_eq!(
            tr(Lang::Russian, "Nothing is selected"),
            "Ничего не выбрано"
        );
    }

    #[test]
    fn unknown_keys_fall_back_to_english() {
        assert_eq!(
            tr(Lang::Russian, "A string nobody translated"),
            "A string nobody translated"
        );
    }

    #[test]
    fn placeholders_are_replaced() {
        let text = tf(Lang::Russian, "Loaded {name}", &[("name", "clip.mp4")]);
        assert_eq!(text, "Загружено: clip.mp4");
        let text = tf(Lang::English, "Loaded {name}", &[("name", "clip.mp4")]);
        assert_eq!(text, "Loaded clip.mp4");
    }

    /// Every string that the user interface translates has to exist in the
    /// Russian table, otherwise the language switch would show English text.
    #[test]
    fn every_used_string_is_translated() {
        let mut missing: Vec<&str> = Vec::new();
        for source in SOURCES {
            for key in used_keys(source) {
                if UNTRANSLATED.contains(&key) {
                    continue;
                }
                if ru(key) == key && !missing.contains(&key) {
                    missing.push(key);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "these strings are missing a Russian translation: {missing:?}"
        );
    }

    /// A translation that is not used any more is dead weight that nobody
    /// keeps up to date.
    #[test]
    fn every_translation_is_used() {
        let mut used: Vec<&str> = Vec::new();
        for source in SOURCES {
            used.extend(used_keys(source));
        }
        used.extend(SHORTCUTS.iter().map(|(_, action)| *action));
        used.extend(UNTRANSLATED.iter().copied());

        let dead: Vec<&str> = TRANSLATIONS
            .iter()
            .map(|(english, _)| *english)
            .filter(|english| !used.contains(english))
            .collect();
        assert!(
            dead.is_empty(),
            "these translations are not used any more: {dead:?}"
        );
    }

    /// The descriptions of the shortcut list are translated as well.
    #[test]
    fn every_shortcut_is_translated() {
        let missing: Vec<&str> = SHORTCUTS
            .iter()
            .map(|(_, action)| *action)
            .filter(|action| ru(action) == *action)
            .collect();
        assert!(
            missing.is_empty(),
            "these shortcuts are missing a Russian translation: {missing:?}"
        );
    }
}
