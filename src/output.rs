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
    let escape = |s: &str| cell(s).replace('\\', "\\\\").replace('|', "\\|");
    let mut rows = vec![
        columns
            .iter()
            .map(|(header, _)| escape(header))
            .collect::<Vec<_>>(),
    ];
    rows.extend(items.iter().map(|item| {
        columns
            .iter()
            .map(|(_, key)| escape(&scalar(&item[*key])))
            .collect::<Vec<_>>()
    }));
    let widths = (0..columns.len())
        .map(|i| {
            rows.iter()
                .map(|row| row[i].chars().count())
                .max()
                .unwrap_or(0)
                .max(3)
        })
        .collect::<Vec<_>>();
    let format_row = |row: &[String]| {
        format!(
            "| {} |",
            row.iter()
                .enumerate()
                .map(|(i, value)| format!(
                    "{}{}",
                    value,
                    " ".repeat(widths[i].saturating_sub(value.chars().count()))
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    let mut lines = vec![
        format_row(&rows[0]),
        format_row(&widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>()),
    ];
    lines.extend(rows.iter().skip(1).map(|row| format_row(row)));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_table_aligns_and_escapes_cells() {
        let result = table(
            &[serde_json::json!({"id":7,"name":"a|b\nc"})],
            &[("ID", "id"), ("NAME", "name")],
        );
        assert!(result.starts_with("| ID  | NAME"));
        assert!(result.contains("| --- |"));
        assert!(result.contains("a\\|b c"));
        assert_eq!(result.lines().count(), 3);
        let empty = table(&[], &[("ID", "id")]);
        assert_eq!(empty, "| ID  |\n| --- |");
    }

    #[test]
    fn removes_terminal_controls() {
        assert_eq!(clean("hi\x1b[31m\r\0bye\n"), "hi[31mbye\n");
        assert_eq!(cell("a\tb\nc"), "a b c");
    }
}
