use serde_json::Value;

// Remote text must not be able to issue terminal escapes, OSC links, or carriage returns.
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
pub fn cell(text: &str) -> String {
    clean(text).replace(['\n', '\t'], " ")
}
pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
pub fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => clean(s),
        Value::Null => String::new(),
        _ if value.is_number() || value.is_boolean() => value.to_string(),
        _ => String::new(),
    }
}
pub fn rows(value: &Value) -> Vec<Value> {
    value["value"]
        .as_array()
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default()
}
pub fn table(items: &[Value], columns: &[(&str, &str)]) -> String {
    let mut result = columns
        .iter()
        .map(|(header, _)| *header)
        .collect::<Vec<_>>()
        .join("\t");
    for item in items {
        result.push('\n');
        result.push_str(
            &columns
                .iter()
                .map(|(_, key)| cell(&scalar(&item[*key])))
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_terminal_controls() {
        assert_eq!(clean("hi\x1b[31m\r\0bye\n"), "hi[31mbye\n");
        assert_eq!(cell("a\tb\nc"), "a b c");
    }
}
