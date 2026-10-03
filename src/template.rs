//! language-core's template compile (`template/compile.ts`): compiler-dom's parser plus a reduced
//! transform pass (`transformVBindShorthand`, the language-core `transformIf` / `transformFor` /
//! `transformElement` / `transformText`, and the side effects of the base directive transforms).

use indexmap::IndexSet;
use vue_sfc::core::ast::{Arena, ElementType, ForNode, IfBranchNode, IfNode, Node, NodeId, SourceLocation};
use vue_sfc::core::parser::base_parse;
use vue_sfc::core::utils::find_prop;
use vue_sfc::dom::parser_options::dom_parser_options;

use crate::shared::camelize;

pub struct TemplateAst {
    pub arena: Arena,
    pub root: NodeId,
    /// `root.components`: component tags in the order `transformElement` exited them
    pub components: Vec<String>,
}

/// `shouldAddSuffixRE = /(?<=<[^>/]+)$/` (#4583): the template ends inside an unclosed tag.
fn should_add_suffix(template: &str) -> bool {
    let tail_start = template.rfind(['>', '/']).map(|i| i + 1).unwrap_or(0);
    let tail = &template[tail_start..];
    // some `<` in the tail with at least one character after it
    tail.char_indices().any(|(i, c)| c == '<' && i + 1 < tail.len())
}

pub fn compile(content: &str) -> TemplateAst {
    let mut source = content.to_string();
    if should_add_suffix(&source) {
        source.push('>');
    }
    let mut options = dom_parser_options();
    options.comments = true;
    let parsed = base_parse(&source, options);
    let mut t = Transform { a: parsed.arena, parent: None, child_index: 0, current: None, removed: Vec::new(), components: IndexSet::new() };
    t.traverse_node(parsed.root);
    TemplateAst { arena: t.a, root: parsed.root, components: t.components.into_iter().collect() }
}

enum Exit {
    Element(NodeId),
    Text(NodeId),
    ForTemplateKey,
}

struct Transform {
    a: Arena,
    parent: Option<NodeId>,
    child_index: usize,
    current: Option<NodeId>,
    /// `onNodeRemoved` counters, one per active `traverseChildren` loop
    removed: Vec<usize>,
    components: IndexSet<String>,
}

fn children_mut(a: &mut Arena, id: NodeId) -> &mut Vec<NodeId> {
    match a.node_mut(id) {
        Node::Root(n) => &mut n.children,
        Node::Element(n) => &mut n.children,
        Node::IfBranch(n) => &mut n.children,
        Node::For(n) => &mut n.children,
        _ => unreachable!("node without children"),
    }
}

fn children(a: &Arena, id: NodeId) -> &Vec<NodeId> {
    match a.node(id) {
        Node::Root(n) => &n.children,
        Node::Element(n) => &n.children,
        Node::IfBranch(n) => &n.children,
        Node::For(n) => &n.children,
        _ => unreachable!("node without children"),
    }
}

fn clone_loc(loc: &SourceLocation) -> SourceLocation {
    loc.clone()
}

impl Transform {
    fn replace_node(&mut self, node: NodeId) {
        let parent = self.parent.expect("replace without parent");
        children_mut(&mut self.a, parent)[self.child_index] = node;
        self.current = Some(node);
    }

    /// `context.removeNode(node?)`
    fn remove_node(&mut self, node: Option<NodeId>) {
        let parent = self.parent.expect("remove without parent");
        let list = children(&self.a, parent);
        let removal_index = match node {
            Some(n) => list.iter().position(|&c| c == n),
            None => self.current.map(|_| self.child_index),
        };
        if node.is_none() || node == self.current {
            self.current = None;
            self.node_removed();
        } else if let Some(ri) = removal_index {
            if self.child_index > ri {
                self.child_index -= 1;
                self.node_removed();
            }
        }
        if let Some(ri) = removal_index {
            children_mut(&mut self.a, parent).remove(ri);
        }
    }

    fn traverse_node(&mut self, node: NodeId) {
        self.current = Some(node);
        let mut node = node;
        let mut exits: Vec<Exit> = Vec::new();

        // transformVBindShorthand
        self.v_bind_shorthand(node);
        // transformIf
        self.structural(node, |n| matches!(n, "if" | "else-if" | "else"), &mut exits, Self::transform_if);
        match self.current {
            None => return,
            Some(c) => node = c,
        }
        // transformFor
        self.structural(node, |n| n == "for", &mut exits, Self::transform_for);
        match self.current {
            None => return,
            Some(c) => node = c,
        }
        // transformElement
        exits.push(Exit::Element(node));
        // transformText
        if matches!(self.a.node(node), Node::Root(_) | Node::Element(_) | Node::For(_) | Node::IfBranch(_)) {
            exits.push(Exit::Text(node));
        }

        match self.a.node(node) {
            Node::If(n) => {
                let branches = n.branches.clone();
                for b in branches {
                    self.traverse_node(b);
                }
            }
            Node::IfBranch(_) | Node::For(_) | Node::Element(_) | Node::Root(_) => self.traverse_children(node),
            _ => {}
        }

        self.current = Some(node);
        while let Some(exit) = exits.pop() {
            match exit {
                Exit::Element(n) => self.element_exit(n),
                Exit::Text(n) => self.text_exit(n),
                Exit::ForTemplateKey => {}
            }
        }
    }

    fn node_removed(&mut self) {
        if let Some(c) = self.removed.last_mut() {
            *c += 1;
        }
    }

    fn traverse_children(&mut self, parent: NodeId) {
        self.removed.push(0);
        let mut i = 0usize;
        while i < children(&self.a, parent).len() {
            let child = children(&self.a, parent)[i];
            if matches!(self.a.node(child), Node::Str(_)) {
                i += 1;
                continue;
            }
            self.parent = Some(parent);
            self.child_index = i;
            *self.removed.last_mut().unwrap() = 0;
            self.traverse_node(child);
            // `onNodeRemoved` decrements this loop's index
            let removed = *self.removed.last().unwrap();
            i = (i + 1).wrapping_sub(removed);
        }
        self.removed.pop();
    }

    fn v_bind_shorthand(&mut self, node: NodeId) {
        let Node::Element(el) = self.a.node(node) else { return };
        let props = el.props.clone();
        for p in props {
            let (arg, has_exp) = match self.a.node(p) {
                Node::Directive(d) if d.name == "bind" => (d.arg, d.exp.is_some()),
                _ => continue,
            };
            let Some(arg) = arg else { continue };
            if has_exp {
                continue;
            }
            let (is_simple_static, content, loc) = match self.a.node(arg) {
                Node::SimpleExpression(e) => (e.is_static, e.content.clone(), e.loc.clone()),
                other => (false, String::new(), other.loc().clone()),
            };
            if !is_simple_static {
                let exp = self.a.create_simple_expression("", true, loc, vue_sfc::core::ast::ConstantType::NotConstant);
                self.a.dir_mut(p).exp = Some(exp);
            } else {
                let prop_name = camelize(&content);
                let ok = prop_name.chars().next().is_some_and(|c| {
                    c.is_ascii_alphabetic() || c == '_' || c == '$' || ('\u{a0}'..='\u{ffff}').contains(&c) || c == '-'
                });
                if ok {
                    let exp = self.a.create_simple_expression(prop_name, false, loc, vue_sfc::core::ast::ConstantType::NotConstant);
                    self.a.dir_mut(p).exp = Some(exp);
                }
            }
        }
    }

    /// language-core's `createStructuralDirectiveTransform`
    fn structural(
        &mut self,
        node: NodeId,
        matches: impl Fn(&str) -> bool,
        exits: &mut Vec<Exit>,
        f: fn(&mut Self, NodeId, NodeId) -> Option<Exit>,
    ) {
        if !matches!(self.a.node(node), Node::Element(_)) {
            return;
        }
        let mut i = 0;
        while i < self.a.el(node).props.len() {
            let p = self.a.el(node).props[i];
            let hit = matches!(self.a.node(p), Node::Directive(d) if matches(&d.name));
            if hit {
                self.a.el_mut(node).props.remove(i);
                if let Some(exit) = f(self, node, p) {
                    exits.push(exit);
                }
                continue;
            }
            i += 1;
        }
    }

    fn create_if_branch(&mut self, node: NodeId, dir: NodeId) -> NodeId {
        let d = self.a.dir(dir);
        let condition = if d.name == "else" { None } else { d.exp };
        let user_key = find_prop(&self.a, node, "key", false, false);
        let loc = self.a.el(node).loc.clone();
        self.a.add(Node::IfBranch(Box::new(IfBranchNode {
            condition,
            children: vec![node],
            user_key,
            is_template_if: false,
            loc,
        })))
    }

    fn transform_if(&mut self, node: NodeId, dir: NodeId) -> Option<Exit> {
        let name = self.a.dir(dir).name.clone();
        if name == "if" {
            let branch = self.create_if_branch(node, dir);
            let loc = clone_loc(&self.a.el(node).loc);
            let if_node = self.a.add(Node::If(Box::new(IfNode { branches: vec![branch], codegen_node: None, loc })));
            self.replace_node(if_node);
            return None;
        }
        let parent = self.parent?;
        let mut comments: Vec<NodeId> = Vec::new();
        let mut i = children(&self.a, parent).iter().position(|&c| c == node).map(|i| i as isize).unwrap_or(-1);
        loop {
            // `while (i-- >= -1)`
            if i < -1 {
                break;
            }
            i -= 1;
            let sibling = if i >= 0 { children(&self.a, parent).get(i as usize).copied() } else { None };
            match sibling.map(|s| (s, self.a.node(s))) {
                Some((s, Node::Comment(_))) => {
                    self.remove_node(Some(s));
                    comments.insert(0, s);
                    continue;
                }
                Some((s, Node::Text(t))) if crate::text::js_trim(&t.content).is_empty() => {
                    self.remove_node(Some(s));
                    continue;
                }
                Some((s, Node::If(_))) => {
                    self.remove_node(None);
                    let branch = self.create_if_branch(node, dir);
                    if !comments.is_empty() {
                        let ch = &mut self.a.branch_mut(branch).children;
                        for (k, c) in comments.iter().enumerate() {
                            ch.insert(k, *c);
                        }
                    }
                    self.a.if_node_mut(s).branches.push(branch);
                    self.traverse_node(branch);
                    self.current = None;
                }
                _ => {}
            }
            break;
        }
        None
    }

    fn transform_for(&mut self, node: NodeId, dir: NodeId) -> Option<Exit> {
        let d = self.a.dir(dir);
        d.exp?;
        let parse_result = d.for_parse_result.clone()?;
        let loc = d.loc.clone();
        let for_node = self.a.add(Node::For(Box::new(ForNode {
            source: parse_result.source,
            value_alias: parse_result.value,
            key_alias: parse_result.key,
            object_index_alias: parse_result.index,
            parse_result,
            children: vec![node],
            codegen_node: None,
            loc,
        })));
        self.replace_node(for_node);
        Some(Exit::ForTemplateKey)
    }

    fn element_exit(&mut self, node: NodeId) {
        let Node::Element(el) = self.a.node(node) else { return };
        if el.tag_type == ElementType::Template {
            return;
        }
        let is_component = el.tag_type == ElementType::Component;
        let props = el.props.clone();
        let tag = el.tag.clone();
        for p in props {
            let Node::Directive(d) = self.a.node(p) else { continue };
            if d.name == "slot" {
                continue;
            }
            if d.arg.is_none() && (d.name == "bind" || d.name == "on") {
                continue;
            }
            if d.name == "bind" {
                self.transform_bind(p);
            }
        }
        if is_component {
            self.components.insert(tag);
        }
    }

    /// The AST mutations of compiler-core's `transformBind`.
    fn transform_bind(&mut self, dir: NodeId) {
        let d = self.a.dir(dir);
        let Some(arg) = d.arg else { return };
        if let Some(exp) = d.exp {
            if let Node::SimpleExpression(e) = self.a.node(exp) {
                if crate::text::js_trim(&e.content).is_empty() {
                    return;
                }
            }
        }
        let modifiers: Vec<String> = d.modifiers.iter().map(|m| self.a.exp(*m).content.clone()).collect();
        let has = |m: &str| modifiers.iter().any(|x| x == m);
        let Node::SimpleExpression(_) = self.a.node(arg) else { return };
        {
            let e = self.a.exp_mut(arg);
            if !e.is_static {
                e.content = if e.content.is_empty() { "\"\"".into() } else { format!("{} || \"\"", e.content) };
            }
        }
        if has("camel") {
            let e = self.a.exp_mut(arg);
            if e.is_static {
                e.content = camelize(&e.content);
            } else {
                e.content = format!("_camelize({})", e.content);
            }
        }
        for (m, prefix) in [("prop", "."), ("attr", "^")] {
            if has(m) {
                let e = self.a.exp_mut(arg);
                if e.is_static {
                    e.content = format!("{prefix}{}", e.content);
                } else {
                    e.content = format!("`{prefix}${{{}}}`", e.content);
                }
            }
        }
    }

    fn text_exit(&mut self, node: NodeId) {
        let is_text = |a: &Arena, id: NodeId| matches!(a.node(id), Node::Interpolation(_) | Node::Text(_));
        let mut i = 0;
        let mut container: Option<NodeId> = None;
        while i < children(&self.a, node).len() {
            let child = children(&self.a, node)[i];
            if is_text(&self.a, child) {
                let j = i + 1;
                while j < children(&self.a, node).len() {
                    let next = children(&self.a, node)[j];
                    if is_text(&self.a, next) {
                        let c = match container {
                            Some(c) => c,
                            None => {
                                let loc = self.a.node(child).loc().clone();
                                let c = self.a.create_compound_expression(vec![child], loc);
                                children_mut(&mut self.a, node)[i] = c;
                                container = Some(c);
                                c
                            }
                        };
                        let plus = self.a.add(Node::Str(" + ".into()));
                        self.a.compound_mut(c).children.push(plus);
                        self.a.compound_mut(c).children.push(next);
                        children_mut(&mut self.a, node).remove(j);
                    } else {
                        container = None;
                        break;
                    }
                }
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix() {
        assert!(should_add_suffix("<div><span"));
        assert!(!should_add_suffix("<div></div>"));
        assert!(!should_add_suffix("<div>\n<"));
        assert!(should_add_suffix("<a <b"));
    }

    #[test]
    fn if_chain() {
        let t = compile("<div v-if=\"a\"/>\n<!-- c -->\n<p v-else-if=\"b\"/>\n<span v-else/>");
        let root = t.arena.root(t.root);
        assert_eq!(root.children.len(), 1);
        let Node::If(n) = t.arena.node(root.children[0]) else { panic!() };
        assert_eq!(n.branches.len(), 3);
        assert_eq!(t.arena.branch(n.branches[1]).children.len(), 2);
    }
}
