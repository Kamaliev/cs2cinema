//! Пользовательские настройки монтажа: реестр, значения по умолчанию, проверка и миграции.
//!
//! Одна точка правды — таблица [`DEFS`]. Чтобы добавить настройку, достаточно дописать в неё строку:
//! - в базе у пользователя хранится только то, что он менял (`{"v":1,"values":{...}}`), поэтому у всех
//!   старых пользователей новая настройка сразу получает значение по умолчанию, миграция базы не нужна;
//! - экран настроек в Telegram строится из [`schema`] и сам покажет новую галку или выбор;
//! - значение доходит до раннера в `Assignment.options` и читается через [`Montage`].
//!
//! Переименование и смена смысла настройки — это шаг в [`MIGRATIONS`] (правит сохранённые значения и поднимает
//! [`VERSION`]); удалённый ключ молча отбрасывается при чтении. Неизвестные ключи от более новой стороны
//! протокола раннер пропускает, как и остальные `options`.

use serde_json::{Map, Value, json};

/// Версия формата сохранённых настроек; растёт вместе с [`MIGRATIONS`].
pub const VERSION: u32 = 1;

/// Шаг миграции: правит сохранённые значения версии `k + 1`, превращая их в значения версии `k + 2`.
pub type Migration = fn(&mut Map<String, Value>);

/// Шаги миграции значений. Пока пуст: формат первой версии.
pub const MIGRATIONS: &[Migration] = &[];

/// Вид настройки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Галка: JSON `true`/`false`.
    Toggle { default: bool },
    /// Выбор из списка целых: пары «значение, подпись».
    Choice {
        default: i64,
        options: &'static [(i64, &'static str)],
    },
}

/// Описание одной настройки.
#[derive(Debug, Clone, Copy)]
pub struct Def {
    pub key: &'static str,
    /// Идентификатор группы из [`GROUPS`].
    pub group: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    pub kind: Kind,
}

impl Def {
    /// Значение по умолчанию в JSON.
    pub fn default_value(&self) -> Value {
        match self.kind {
            Kind::Toggle { default } => Value::Bool(default),
            Kind::Choice { default, .. } => json!(default),
        }
    }

    /// Годится ли `v` как значение этой настройки.
    pub fn accepts(&self, v: &Value) -> bool {
        match self.kind {
            Kind::Toggle { .. } => v.is_boolean(),
            Kind::Choice { options, .. } => v.as_i64().is_some_and(|n| options.iter().any(|(o, _)| *o == n)),
        }
    }
}

/// Группы экрана настроек в порядке показа: идентификатор и заголовок.
pub const GROUPS: &[(&str, &str)] = &[("content", "Что в ролике"), ("quality", "Качество записи")];

/// Все настройки. Порядок — порядок на экране внутри группы.
pub const DEFS: &[Def] = &[
    Def {
        key: "detailed",
        group: "content",
        title: "Подробный ролик",
        hint: "Все убийства и все ракурсы. Ролик получается примерно вдвое длиннее.",
        kind: Kind::Toggle { default: false },
    },
    Def {
        key: "flybys",
        group: "content",
        title: "Пролёты камеры",
        hint: "Камера облетает место действия между моментами.",
        kind: Kind::Toggle { default: true },
    },
    Def {
        key: "walls",
        group: "content",
        title: "Учитывать стены",
        hint: "Камера не залетает в стены. Выключите, если пролёты выглядят странно.",
        kind: Kind::Toggle { default: true },
    },
    Def {
        key: "chronological",
        group: "content",
        title: "По порядку раундов",
        hint: "Моменты идут в хронологии, а не от лучших к менее эффектным.",
        kind: Kind::Toggle { default: false },
    },
    Def {
        key: "fps",
        group: "quality",
        title: "Кадров в секунду",
        hint: "30 пишется быстрее, 60 плавнее.",
        kind: Kind::Choice {
            default: 60,
            options: &[(30, "30"), (60, "60")],
        },
    },
    Def {
        key: "resolution",
        group: "quality",
        title: "Разрешение",
        hint: "Высота кадра. 1080p пишется заметно дольше.",
        kind: Kind::Choice {
            default: 720,
            options: &[(720, "720p"), (1080, "1080p")],
        },
    },
];

fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// Значения по умолчанию для всех настроек.
pub fn defaults() -> Map<String, Value> {
    DEFS.iter().map(|d| (d.key.to_owned(), d.default_value())).collect()
}

/// Читает сохранённую строку: пустая или битая — «ничего не менял». Старые версии проходят [`MIGRATIONS`],
/// ключи, которых больше нет в реестре, и значения неподходящего вида отбрасываются.
pub fn parse_stored(raw: &str) -> Map<String, Value> {
    parse_stored_with(raw, MIGRATIONS)
}

/// То же с заданным списком миграций (для тестов).
pub fn parse_stored_with(raw: &str, migrations: &[Migration]) -> Map<String, Value> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(raw) else {
        return Map::new();
    };
    let from = root.get("v").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let Some(Value::Object(mut values)) = root.get("values").cloned() else {
        return Map::new();
    };
    for step in migrations.iter().skip(from - 1) {
        step(&mut values);
    }
    values.retain(|k, v| def(k).is_some_and(|d| d.accepts(v)));
    values
}

/// Сериализует сохранённые значения в строку для базы.
pub fn to_stored(values: &Map<String, Value>) -> String {
    json!({ "v": VERSION, "values": values }).to_string()
}

/// Полный набор значений: сохранённое поверх умолчаний.
pub fn resolve(raw: &str) -> Map<String, Value> {
    let mut all = defaults();
    all.extend(parse_stored(raw));
    all
}

/// Проверяет запрос на изменение: `{"ключ": значение}`; `null` сбрасывает настройку к умолчанию. Неизвестный
/// ключ и значение не того вида — ошибка с понятным текстом.
pub fn check_patch(patch: &Value) -> Result<Map<String, Value>, String> {
    let Value::Object(map) = patch else {
        return Err("ожидался объект вида {\"ключ\": значение}".to_owned());
    };
    for (k, v) in map {
        let Some(d) = def(k) else {
            return Err(format!("неизвестная настройка «{k}»"));
        };
        if !v.is_null() && !d.accepts(v) {
            return Err(format!("настройка «{}»: недопустимое значение", d.title));
        }
    }
    Ok(map.clone())
}

/// Накладывает проверенное изменение на сохранённую строку и возвращает новую.
pub fn apply_patch(raw: &str, patch: &Value) -> Result<String, String> {
    let patch = check_patch(patch)?;
    let mut values = parse_stored(raw);
    for (k, v) in patch {
        if v.is_null() {
            values.remove(&k);
        } else {
            values.insert(k, v);
        }
    }
    Ok(to_stored(&values))
}

/// Описание экрана для интерфейса: группы, настройки с видом, подсказкой и умолчанием.
pub fn schema() -> Value {
    let groups: Vec<Value> = GROUPS
        .iter()
        .map(|(id, title)| {
            let items: Vec<Value> = DEFS
                .iter()
                .filter(|d| d.group == *id)
                .map(|d| {
                    let mut o = json!({"key": d.key, "title": d.title, "hint": d.hint, "default": d.default_value()});
                    match d.kind {
                        Kind::Toggle { .. } => o["kind"] = json!("toggle"),
                        Kind::Choice { options, .. } => {
                            o["kind"] = json!("choice");
                            o["options"] = options.iter().map(|(v, t)| json!({"value": v, "title": t})).collect();
                        }
                    }
                    o
                })
                .collect();
            json!({"id": id, "title": title, "items": items})
        })
        .collect();
    json!({ "version": VERSION, "groups": groups })
}

/// Одна строка про каждую настройку для текстового ответа бота.
pub fn describe(values: &Map<String, Value>) -> Vec<String> {
    DEFS.iter()
        .map(|d| {
            let v = values.get(d.key).cloned().unwrap_or_else(|| d.default_value());
            let shown = match (d.kind, &v) {
                (Kind::Toggle { .. }, Value::Bool(true)) => "вкл".to_owned(),
                (Kind::Toggle { .. }, _) => "выкл".to_owned(),
                (Kind::Choice { options, .. }, v) => options
                    .iter()
                    .find(|(o, _)| Some(*o) == v.as_i64())
                    .map(|(_, t)| (*t).to_owned())
                    .unwrap_or_default(),
            };
            format!("{}: {shown}", d.title)
        })
        .collect()
}

/// Типизированные настройки монтажа, как их читает раннер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Montage {
    pub detailed: bool,
    pub flybys: bool,
    pub walls: bool,
    pub chronological: bool,
    pub fps: u32,
    /// Высота кадра.
    pub resolution: u32,
}

impl Montage {
    /// Читает `Assignment.options`: чего нет или что не подходит, берётся по умолчанию.
    pub fn from_options(options: &Map<String, Value>) -> Montage {
        let mut all = defaults();
        all.extend(options.iter().filter(|(k, v)| def(k).is_some_and(|d| d.accepts(v))).map(|(k, v)| (k.clone(), v.clone())));
        let b = |k: &str| all.get(k).and_then(Value::as_bool).unwrap_or_default();
        let n = |k: &str| all.get(k).and_then(Value::as_i64).unwrap_or_default();
        Montage {
            detailed: b("detailed"),
            flybys: b("flybys"),
            walls: b("walls"),
            chronological: b("chronological"),
            fps: n("fps") as u32,
            resolution: n("resolution") as u32,
        }
    }

    /// Размер кадра 16:9 по высоте.
    pub fn frame(&self) -> (u32, u32) {
        let h = self.resolution.clamp(360, 2160) / 2 * 2;
        (h * 16 / 9 / 2 * 2, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_stored_gives_the_defaults() {
        for raw in ["", "мусор", "[]", "{}", r#"{"v":1}"#] {
            assert_eq!(resolve(raw), defaults(), "{raw:?}");
        }
        let m = Montage::from_options(&resolve(""));
        assert_eq!((m.fps, m.frame(), m.flybys, m.detailed), (60, (1280, 720), true, false));
    }

    #[test]
    fn stored_values_override_defaults_and_bad_ones_are_dropped() {
        let raw = r#"{"v":1,"values":{"detailed":true,"fps":30,"resolution":999,"flybys":"да","удалённая":true}}"#;
        let all = resolve(raw);
        assert_eq!(all["detailed"], json!(true));
        assert_eq!(all["fps"], json!(30));
        assert_eq!(all["resolution"], json!(720), "999 нет в списке — умолчание");
        assert_eq!(all["flybys"], json!(true), "строка вместо галки — умолчание");
        assert!(!all.contains_key("удалённая"));
    }

    #[test]
    fn patch_sets_resets_and_rejects() {
        let raw = apply_patch("", &json!({"detailed": true, "fps": 30})).unwrap();
        assert_eq!(resolve(&raw)["fps"], json!(30));
        let raw = apply_patch(&raw, &json!({"fps": null})).unwrap();
        assert_eq!(resolve(&raw)["fps"], json!(60));
        assert_eq!(resolve(&raw)["detailed"], json!(true), "соседние значения не тронуты");
        assert!(apply_patch(&raw, &json!({"нет такой": true})).unwrap_err().contains("неизвестная"));
        assert!(apply_patch(&raw, &json!({"fps": 45})).is_err());
        assert!(apply_patch(&raw, &json!({"detailed": 1})).is_err());
        assert!(apply_patch(&raw, &json!([1])).is_err());
    }

    #[test]
    fn migrations_run_from_the_stored_version() {
        fn rename(m: &mut Map<String, Value>) {
            if let Some(v) = m.remove("old_detailed") {
                m.insert("detailed".to_owned(), v);
            }
        }
        let steps: &[Migration] = &[rename];
        let old = parse_stored_with(r#"{"v":1,"values":{"old_detailed":true}}"#, steps);
        assert_eq!(old["detailed"], json!(true));
        let current = parse_stored_with(r#"{"v":2,"values":{"old_detailed":true}}"#, steps);
        assert!(current.is_empty(), "версия 2 уже мигрирована, старый ключ отбрасывается");
    }

    #[test]
    fn registry_is_consistent() {
        let mut keys: Vec<_> = DEFS.iter().map(|d| d.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), DEFS.len(), "ключи уникальны");
        assert_eq!(MIGRATIONS.len() as u32 + 1, VERSION, "версия равна числу шагов + 1");
        for d in DEFS {
            assert!(GROUPS.iter().any(|(g, _)| *g == d.group), "{}: нет группы", d.key);
            assert!(d.accepts(&d.default_value()), "{}: умолчание не проходит проверку", d.key);
        }
        let s = schema();
        assert_eq!(s["groups"].as_array().unwrap().iter().map(|g| g["items"].as_array().unwrap().len()).sum::<usize>(), DEFS.len());
    }

    #[test]
    fn unknown_option_keys_are_ignored_by_the_runner_side() {
        let mut o = resolve("");
        o.insert("из_будущего".to_owned(), json!(5));
        o.insert("fps".to_owned(), json!(7));
        assert_eq!(Montage::from_options(&o).fps, 60);
    }
}
