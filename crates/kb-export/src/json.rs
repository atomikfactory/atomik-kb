//! JSON text with a fixed key order and `serde_json`'s pretty layout (two-space indent, `"key": value`).
//!
//! `serde_json::Map` sorts its keys, or keeps insertion order when any crate of the build turns on
//! `preserve_order`; the files written here (the sidecar, `.excalidraw` scenes) must read the same
//! whichever, so their objects are written from ordered pairs.

use serde_json::Value;

/// A JSON document with the order of its objects' keys fixed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// A string, number, boolean or null.
    Scalar(Value),
    /// An object, keys in this order.
    Object(Vec<(String, Node)>),
    /// An array.
    Array(Vec<Node>),
}

impl Node {
    /// A string.
    pub(crate) fn string(text: impl Into<String>) -> Self {
        Node::Scalar(Value::String(text.into()))
    }

    /// An object from `(key, node)` pairs.
    pub(crate) fn object<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Node)>) -> Self {
        Node::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Any JSON value, with the keys of its objects sorted.
    pub(crate) fn sorted(value: &Value) -> Self {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                Node::Object(
                    keys.into_iter()
                        .filter_map(|key| Some((key.clone(), Node::sorted(map.get(key)?))))
                        .collect(),
                )
            }
            Value::Array(items) => Node::Array(items.iter().map(Node::sorted).collect()),
            scalar => Node::Scalar(scalar.clone()),
        }
    }

    /// The pretty text of the document (no trailing newline).
    pub(crate) fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Node::Scalar(value) => out.push_str(&value.to_string()),
            Node::Object(pairs) if pairs.is_empty() => out.push_str("{}"),
            Node::Array(items) if items.is_empty() => out.push_str("[]"),
            Node::Object(pairs) => {
                out.push_str("{\n");
                for (index, (key, node)) in pairs.iter().enumerate() {
                    indent(out, depth + 1);
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push_str(": ");
                    node.write(out, depth + 1);
                    out.push_str(if index + 1 == pairs.len() {
                        "\n"
                    } else {
                        ",\n"
                    });
                }
                indent(out, depth);
                out.push('}');
            }
            Node::Array(items) => {
                out.push_str("[\n");
                for (index, node) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    node.write(out, depth + 1);
                    out.push_str(if index + 1 == items.len() {
                        "\n"
                    } else {
                        ",\n"
                    });
                }
                indent(out, depth);
                out.push(']');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_layout_is_serde_json_pretty_with_the_given_key_order() {
        let node = Node::object([
            ("zeta", Node::Scalar(json!(1))),
            (
                "alpha",
                Node::Array(vec![Node::string("x"), Node::Scalar(json!(null))]),
            ),
            ("empty", Node::Array(vec![])),
            ("map", Node::object([("k", Node::Scalar(json!(true)))])),
            ("none", Node::Object(vec![])),
        ]);
        assert_eq!(
            node.pretty(),
            "{\n  \"zeta\": 1,\n  \"alpha\": [\n    \"x\",\n    null\n  ],\n  \"empty\": [],\n  \"map\": {\n    \"k\": true\n  },\n  \"none\": {}\n}"
        );
    }

    #[test]
    fn sorted_values_read_back_and_print_like_serde_json() {
        let value = json!({ "b": [1, { "y": 1, "x": "é\n" }], "a": null, "c": 2.5 });
        let node = Node::sorted(&value);
        let text = node.pretty();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
        assert_eq!(
            text,
            "{\n  \"a\": null,\n  \"b\": [\n    1,\n    {\n      \"x\": \"é\\n\",\n      \"y\": 1\n    }\n  ],\n  \"c\": 2.5\n}"
        );
    }
}
