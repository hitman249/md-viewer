use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use comrak::{
    markdown_to_html_with_plugins, Options,
    options::{Plugins, RenderPlugins},
    plugins::syntect::SyntectAdapter,
};
use serde::Serialize;
use tauri::State;

/// Открывает URL во внешнем браузере по умолчанию в системе.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    opener::open_browser(&url).map(|_| ()).map_err(|err| format!("Не удалось открыть ссылку: {err}"))
}

/// Открывает локальный .md файл в новом экземпляре программы.
/// Относительный путь разрешается относительно директории текущего документа.
#[tauri::command]
fn open_md(state: State<AppState>, path: String) -> Result<(), String> {
    let md_path = state
        .md_path
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?
        .clone()
        .ok_or_else(|| "Не передан путь к .md файлу".to_string())?;

    let target = PathBuf::from(&path);
    let resolved = if target.is_absolute() {
        target
    } else {
        // Базой для относительных ссылок служит папка открытого документа.
        let base = md_path
            .parent()
            .ok_or_else(|| "Нет базовой директории для ссылки".to_string())?;
        base.join(&target)
    };
    let resolved = resolved
        .canonicalize()
        .map_err(|err| format!("Файл не найден: {err}"))?;

    // Запускаем новый экземпляр этого же исполняемого файла с путём к .md файлу.
    let exe = std::env::current_exe()
        .map_err(|err| format!("Не найден исполняемый файл: {err}"))?;
    std::process::Command::new(exe)
        .arg(&resolved)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Не удалось открыть новый экземпляр: {err}"))
}

/// Состояние приложения: хранит путь к открываемому .md файлу.
/// Mutex нужен, т.к. команды Tauri приходят из асинхронных коллбеков вебвью.
struct AppState {
    md_path: Mutex<Option<PathBuf>>,
}

/// Достаёт первый аргумент командной строки и приводит его к абсолютному пути.
/// canonicalize разрешает относительный путь и заодно проверяет существование файла.
fn first_arg_path() -> Option<PathBuf> {
    let raw: PathBuf = PathBuf::from(std::env::args().nth(1)?);
    Some(raw.canonicalize().unwrap_or(raw))
}

/// Встроенные грамматики syntect не содержат TypeScript, поэтому алиасим языки
/// фенсов кода typescript/ts/tsx на грамматику JavaScript (TS — надмножество JS).
/// Переписываем только токен языка в строке открывающего фенса, сохраняя отступ,
/// маркер (```/~~~) и остаток info-строки; закрывающие фенсы не трогаем.
fn alias_code_languages(source: &str) -> String {
    const ALIASES: [(&str, &str); 5] = [
        ("typescript", "js"), ("ts", "js"), ("tsx", "js"),
        // "shell" — отдельной грамматики нет, красим как bash;
        // "dotenv" (KEY=VALUE) — грамматики нет, красим как properties.
        ("shell", "bash"), ("dotenv", "properties"),
    ];

    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let indent_len = line.len() - line.trim_start().len();
        let trimmed = &line[indent_len..];
        let (marker, rest) = if trimmed.starts_with("```") {
            ("```", &trimmed[3..])
        } else if trimmed.starts_with("~~~") {
            ("~~~", &trimmed[3..])
        } else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        let rest_trim = rest.trim_start();
        // Пустая info-строка — закрывающий фенс, пропускаем.
        if rest_trim.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Первый токен языка до пробела или запятой.
        let token_end = rest_trim
            .char_indices()
            .find(|(_, c)| c.is_whitespace() || *c == ',')
            .map(|(i, _)| i)
            .unwrap_or(rest_trim.len());
        let token = &rest_trim[..token_end];
        let lower = token.to_lowercase();

        let alias = ALIASES.iter().find(|(k, _)| lower == *k).map(|(_, v)| *v);
        match alias {
            Some(repl) => {
                let new_info = format!("{}{}", repl, &rest_trim[token_end..]);
                out.push_str(&line[..indent_len]);
                out.push_str(marker);
                out.push_str(&new_info);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Результат чтения файла: имя документа (для title окна) и готовый HTML.
#[derive(Serialize)]
struct MarkdownDoc {
    name: String,
    html: String,
}

/// Читает markdown-файл по сохранённому пути и возвращает имя + готовый HTML.
#[tauri::command]
fn read_markdown(state: State<AppState>) -> Result<MarkdownDoc, String> {
    // Извлекаем клонированный путь, освобождая блокировку до чтения файла с диска,
    // чтобы не держать Mutex во время медленного ввода-вывода.
    let md_path = state
        .md_path
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?
        .clone()
        .ok_or_else(|| "Не передан путь к .md файлу".to_string())?;

    let source = fs::read_to_string(&md_path)
        .map_err(|err| format!("Не удалось прочитать файл {}: {}", md_path.display(), err))?;

    // Применяем алиасы языков кода (typescript/ts/tsx → js) до рендера.
    let source = alias_code_languages(&source);

    // comrak — CommonMark/GFM-совместимый парсер; рендер на стороне Rust
    // компактнее и производительнее, чем парсинг в браузере.
    // Включаем GFM-расширения: таблицы, зачёркнутый текст, task-списки, автоссылки.
    let mut opts = Options::default();
    opts.extension.strikethrough = true;
    opts.extension.tagfilter = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;

    // Подсветка синтаксиса блоков кода: SyntectAdapter с классами (без встроенных
    // цветов — стили задаём в CSS отдельно, под светлую/тёмную тему).
    let adapter = SyntectAdapter::new(None);
    let plugins = Plugins {
        render: RenderPlugins {
            codefence_syntax_highlighter: Some(&adapter),
            ..RenderPlugins::default()
        },
        ..Plugins::default()
    };

    // Имя файла для заголовка окна: последний компонент пути.
    let name = md_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Markdown".to_string());

    Ok(MarkdownDoc {
        name,
        html: markdown_to_html_with_plugins(&source, &opts, &plugins),
    })
}

/// Точка входа приложения: регистрирует состояние и команды Tauri.
pub fn run() {
    let md_path = first_arg_path();

    tauri::Builder::default()
        .manage(AppState {
            md_path: Mutex::new(md_path),
        })
        .invoke_handler(tauri::generate_handler![read_markdown, open_url, open_md])
        .run(tauri::generate_context!())
        .expect("Ошибка запуска tauri-приложения");
}
