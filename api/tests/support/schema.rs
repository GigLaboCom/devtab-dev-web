//! A validator for the JSON Schema keywords the contract's schemas use
//! (draft 2020-12): `type`, `const`, `enum`, `properties`, `required`,
//! `additionalProperties`, `pattern`, `minLength`, `maxLength`, `minimum`,
//! `maximum`, `allOf`, `anyOf`, `not`, `if`/`then`/`else`, plus the
//! annotations `$schema`, `title`, `description`. Any other keyword fails the
//! check, so a schema can never rely on a rule that is silently skipped.

use serde_json::{Map, Value};

use super::repo;

const ANNOTATIONS: [&str; 3] = ["$schema", "title", "description"];
const KEYWORDS: [&str; 16] = [
    "type",
    "const",
    "enum",
    "properties",
    "required",
    "additionalProperties",
    "pattern",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "allOf",
    "anyOf",
    "not",
    "if",
    "then",
];

/// A schema of `contract/licence-v1/`.
pub fn load(name: &str) -> Value {
    let text = std::fs::read_to_string(repo("contract/licence-v1").join(name)).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The problems of `value` against `schema`; empty when it is valid.
pub fn problems(schema: &Value, value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    check(schema, value, "$", &mut found);
    found
}

/// Panics with the problems unless `value` is valid against the named schema.
pub fn assert_valid(name: &str, value: &Value) {
    let found = problems(&load(name), value);
    assert!(found.is_empty(), "{name}: {found:?} in {value}");
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        other => panic!("unknown type {other}"),
    }
}

fn valid(schema: &Value, value: &Value) -> bool {
    problems(schema, value).is_empty()
}

#[allow(clippy::too_many_lines)] // one keyword after another
fn check(schema: &Value, value: &Value, at: &str, found: &mut Vec<String>) {
    let rules: &Map<String, Value> = schema.as_object().expect("a schema is an object");
    for keyword in rules.keys() {
        assert!(
            ANNOTATIONS.contains(&keyword.as_str())
                || KEYWORDS.contains(&keyword.as_str())
                || keyword == "else",
            "the schema uses `{keyword}`, which this validator does not know"
        );
    }
    if let Some(types) = rules.get("type") {
        let names: Vec<&str> = match types {
            Value::String(name) => vec![name.as_str()],
            Value::Array(list) => list.iter().map(|t| t.as_str().unwrap()).collect(),
            _ => panic!("type is a string or a list"),
        };
        if !names.iter().any(|name| type_matches(name, value)) {
            found.push(format!("{at}: not of type {names:?}"));
            return;
        }
    }
    if let Some(expected) = rules.get("const")
        && value != expected
    {
        found.push(format!("{at}: not {expected}"));
    }
    if let Some(options) = rules.get("enum")
        && !options.as_array().unwrap().contains(value)
    {
        found.push(format!("{at}: not one of {options}"));
    }
    if let Some(text) = value.as_str() {
        let length = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        if let Some(min) = rules.get("minLength").and_then(Value::as_u64)
            && length < min
        {
            found.push(format!("{at}: shorter than {min}"));
        }
        if let Some(max) = rules.get("maxLength").and_then(Value::as_u64)
            && length > max
        {
            found.push(format!("{at}: longer than {max}"));
        }
        if let Some(pattern) = rules.get("pattern").and_then(Value::as_str)
            && !regex::Regex::new(pattern).unwrap().is_match(text)
        {
            found.push(format!("{at}: does not match {pattern}"));
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(min) = rules.get("minimum").and_then(Value::as_f64)
            && number < min
        {
            found.push(format!("{at}: below {min}"));
        }
        if let Some(max) = rules.get("maximum").and_then(Value::as_f64)
            && number > max
        {
            found.push(format!("{at}: above {max}"));
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = rules.get("required") {
            for name in required.as_array().unwrap() {
                let name = name.as_str().unwrap();
                if !object.contains_key(name) {
                    found.push(format!("{at}: {name} is missing"));
                }
            }
        }
        let properties = rules.get("properties").and_then(Value::as_object);
        for (name, item) in object {
            match properties.and_then(|p| p.get(name)) {
                Some(rule) => check(rule, item, &format!("{at}.{name}"), found),
                None if rules.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    found.push(format!("{at}: {name} is not allowed"));
                }
                None => {}
            }
        }
    }
    if let Some(all) = rules.get("allOf") {
        for rule in all.as_array().unwrap() {
            check(rule, value, at, found);
        }
    }
    if let Some(any) = rules.get("anyOf")
        && !any
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| valid(rule, value))
    {
        found.push(format!("{at}: matches none of anyOf"));
    }
    if let Some(rule) = rules.get("not")
        && valid(rule, value)
    {
        found.push(format!("{at}: matches what `not` forbids"));
    }
    if let Some(condition) = rules.get("if") {
        let branch = if valid(condition, value) {
            "then"
        } else {
            "else"
        };
        if let Some(rule) = rules.get(branch) {
            check(rule, value, at, found);
        }
    }
}
