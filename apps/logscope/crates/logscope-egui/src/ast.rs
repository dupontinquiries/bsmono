#![allow(dead_code)]
//! ast.rs — Rust sub-file structure via tree-sitter. Extracts items (fn/struct/enum/trait/impl/mod/…)
//! with name + line/byte span, including methods inside impls and items inside mods. These are the
//! sub-file units the reasoning-dimension comparisons run over: file a vs b, a-v1 vs a-v2, task A vs B.
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind { Fn, Struct, Enum, Trait, Impl, Mod, Const, Static, TypeAlias, Union, Macro, Other }

#[derive(Debug, Clone)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub parent: Option<String>, // enclosing impl type / mod, if any
    pub start_line: usize,      // 0-based
    pub end_line: usize,
    pub start_byte: usize,      // for content-hashing the span with the vault's hasher
    pub end_byte: usize,
}

fn kind_of(k: &str) -> Kind {
    match k {
        "function_item" => Kind::Fn, "struct_item" => Kind::Struct, "enum_item" => Kind::Enum,
        "trait_item" => Kind::Trait, "impl_item" => Kind::Impl, "mod_item" => Kind::Mod,
        "const_item" => Kind::Const, "static_item" => Kind::Static, "type_item" => Kind::TypeAlias,
        "union_item" => Kind::Union, "macro_definition" => Kind::Macro, _ => Kind::Other,
    }
}
fn text<'a>(n: Node, src: &'a str) -> &'a str { n.utf8_text(src.as_bytes()).unwrap_or("") }
fn name_of(n: Node, src: &str) -> String {
    n.child_by_field_name("name").map(|c| text(c, src).to_string()).unwrap_or_default()
}
fn item(n: Node, kind: Kind, name: String, parent: Option<String>) -> Item {
    Item { kind, name, parent,
        start_line: n.start_position().row, end_line: n.end_position().row,
        start_byte: n.start_byte(), end_byte: n.end_byte() }
}

fn walk(node: Node, src: &str, out: &mut Vec<Item>, parent: Option<&str>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        let k = child.kind();
        match k {
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "mod_item"
            | "const_item" | "static_item" | "type_item" | "union_item" | "macro_definition" => {
                let name = name_of(child, src);
                out.push(item(child, kind_of(k), name.clone(), parent.map(|s| s.to_string())));
                if k == "mod_item" || k == "trait_item" {
                    if let Some(body) = child.child_by_field_name("body") { walk(body, src, out, Some(&name)); }
                }
            }
            "impl_item" => {
                let ty = child.child_by_field_name("type").map(|n| text(n, src).to_string()).unwrap_or_else(|| "?".into());
                let label = match child.child_by_field_name("trait") {
                    Some(t) => format!("{} for {}", text(t, src), ty),
                    None => ty.clone(),
                };
                out.push(item(child, Kind::Impl, label, parent.map(|s| s.to_string())));
                if let Some(body) = child.child_by_field_name("body") { walk(body, src, out, Some(&ty)); }
            }
            _ => {}
        }
    }
}

pub fn extract(src: &str) -> Vec<Item> {
    let mut p = Parser::new();
    if p.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() { return Vec::new(); }
    let Some(tree) = p.parse(src, None) else { return Vec::new() };
    let mut out = Vec::new();
    walk(tree.root_node(), src, &mut out, None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const SRC: &str = r#"
pub struct Grid { cells: Vec<u8>, w: usize }
impl Grid {
    pub fn new(w: usize) -> Self { Self { cells: vec![], w } }
    pub fn get(&self, x: usize) -> u8 { self.cells[x] }
}
enum Dir { N, S }
fn helper() -> i32 { 42 }
mod inner { pub fn deep() {} }
"#;

    #[test]
    fn extracts_items_with_names_and_parents() {
        let items = extract(SRC);
        let by = |k: Kind, n: &str| items.iter().any(|i| i.kind == k && i.name == n);
        assert!(by(Kind::Struct, "Grid"));
        assert!(by(Kind::Enum, "Dir"));
        assert!(by(Kind::Fn, "helper"));
        assert!(by(Kind::Mod, "inner"));
        // impl methods carry their impl type as parent
        let new = items.iter().find(|i| i.kind == Kind::Fn && i.name == "new").expect("new()");
        assert_eq!(new.parent.as_deref(), Some("Grid"));
        let get = items.iter().find(|i| i.kind == Kind::Fn && i.name == "get").expect("get()");
        assert_eq!(get.parent.as_deref(), Some("Grid"));
        // nested mod fn
        let deep = items.iter().find(|i| i.name == "deep").expect("deep()");
        assert_eq!(deep.parent.as_deref(), Some("inner"));
        // spans are sane
        assert!(new.start_line < new.end_line || new.start_byte < new.end_byte);
    }

    #[test]
    fn version_change_detectable_by_span_text() {
        // a-v1 vs a-v2: same item name, different body → caller hashes the span to detect change
        let v1 = extract("fn render() -> u8 { 0 }");
        let v2 = extract("fn render() -> u8 { 1 }");
        let r1 = &v1[0]; let r2 = &v2[0];
        assert_eq!(r1.name, r2.name);
        let b1 = "fn render() -> u8 { 0 }"; let b2 = "fn render() -> u8 { 1 }";
        assert_ne!(&b1[r1.start_byte..r1.end_byte], &b2[r2.start_byte..r2.end_byte]);
    }
}
