//! Package-qualified type references in expressions.
//!
//! Haxe lets an expression name a type by its full path without importing it:
//! `shim.IntMath.mul(a, b)`, `core.Kind.Halt`, `mem.Memory.RAM_SIZE`. The rest of the
//! pipeline recognises a type in expression position only as a bare identifier, and
//! derives `#include`s from imports, so a qualified reference used to be emitted
//! member by member with dots (`shim.IntMath.mul(...)`), which is not C++.
//!
//! This pass runs once, after the types are indexed and before imports are resolved.
//! Every expression that is a pure identifier chain starting with a package path
//! (`pack.sub.Type.rest…`) is rewritten to start at the bare type name
//! (`Type.rest…`), and the type's module is added to the file's imports. From there
//! the reference is an ordinary imported one: resolution, `::` qualification and the
//! include all follow the existing rules.
//!
//! It is conservative on purpose. A chain is left alone when its first segment could
//! be a value instead of a package (a parameter, local, loop or catch variable, or a
//! field or method of the enclosing class, anywhere in the function), and when the
//! bare type name would not resolve back to the same type from this module (another
//! type of that name in the module's own package, or in more than one package).

use super::{Module, Program, TypeInfo};
use crate::ast::*;
use std::collections::BTreeSet;

fn is_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn is_lower(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
}

/// The segments of a pure `a.b.c` chain, innermost first; `None` for anything else.
fn chain(e: &Expr) -> Option<Vec<String>> {
    match e {
        Expr::Ident(n) => Some(vec![n.clone()]),
        Expr::Field(recv, name) => {
            let mut v = chain(recv)?;
            v.push(name.clone());
            Some(v)
        }
        _ => None,
    }
}

fn rebuild(segs: &[String]) -> Expr {
    let mut e = Expr::Ident(segs[0].clone());
    for s in &segs[1..] {
        e = Expr::Field(Box::new(e), s.clone());
    }
    e
}

// ---- names that could shadow a package, per function ---------------------------------

fn locals_in_stmts(stmts: &[Stmt], out: &mut BTreeSet<String>) {
    for s in stmts {
        locals_in_stmt(s, out);
    }
}

fn locals_in_stmt(s: &Stmt, out: &mut BTreeSet<String>) {
    match s {
        Stmt::Var { name, init, .. } => {
            out.insert(name.clone());
            if let Some(e) = init {
                locals_in_expr(e, out);
            }
        }
        Stmt::Expr(e, _) | Stmt::Throw(e, _) => locals_in_expr(e, out),
        Stmt::Return(e, _) => {
            if let Some(e) = e {
                locals_in_expr(e, out);
            }
        }
        Stmt::If { cond, then, els, .. } => {
            locals_in_expr(cond, out);
            locals_in_stmt(then, out);
            if let Some(e) = els {
                locals_in_stmt(e, out);
            }
        }
        Stmt::For {
            var,
            value_var,
            iter,
            body,
            ..
        } => {
            out.insert(var.clone());
            if let Some(v) = value_var {
                out.insert(v.clone());
            }
            locals_in_iter(iter, out);
            locals_in_stmt(body, out);
        }
        Stmt::While { cond, body, .. } => {
            locals_in_expr(cond, out);
            locals_in_stmt(body, out);
        }
        Stmt::Switch {
            subject,
            cases,
            default,
            ..
        } => {
            locals_in_expr(subject, out);
            for c in cases {
                locals_in_stmts(&c.body, out);
            }
            if let Some(d) = default {
                locals_in_stmts(d, out);
            }
        }
        Stmt::Try { body, catches, .. } => {
            locals_in_stmt(body, out);
            for c in catches {
                out.insert(c.name.clone());
                locals_in_stmts(&c.body, out);
            }
        }
        Stmt::Block(b) => locals_in_stmts(b, out),
        Stmt::Break | Stmt::Continue | Stmt::Verbatim { .. } => {}
    }
}

fn locals_in_iter(it: &Iterable, out: &mut BTreeSet<String>) {
    match it {
        Iterable::Range(a, b) => {
            locals_in_expr(a, out);
            locals_in_expr(b, out);
        }
        Iterable::Coll(e) => locals_in_expr(e, out),
    }
}

/// Only the binding forms inside expressions matter here: lambdas, comprehensions and
/// block expressions can declare names too.
fn locals_in_expr(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Lambda { params, body, .. } => {
            for p in params {
                out.insert(p.name.clone());
            }
            match &**body {
                LambdaBody::Expr(x) => locals_in_expr(x, out),
                LambdaBody::Block(b) => locals_in_stmts(b, out),
            }
        }
        Expr::Comprehension {
            var,
            value_var,
            iter,
            ..
        } => {
            out.insert(var.clone());
            if let Some(v) = value_var {
                out.insert(v.clone());
            }
            locals_in_iter(iter, out);
        }
        Expr::Block(b) => locals_in_stmts(b, out),
        Expr::Switch { cases, default, .. } => {
            for c in cases {
                locals_in_stmts(&c.body, out);
            }
            if let Some(d) = default {
                locals_in_stmts(d, out);
            }
        }
        _ => {}
    }
}

// ---- the rewrite -----------------------------------------------------------------------

struct Ctx<'a> {
    types: &'a [TypeInfo],
    modules: &'a [Module],
    package: Vec<String>,
    /// Names that could be values here; a chain starting with one is not a package path.
    shadows: BTreeSet<String>,
    /// Import paths to add to the file (module paths, not type paths).
    imports: BTreeSet<Vec<String>>,
}

impl Ctx<'_> {
    /// The type a leading package path names, if the chain is one and rewriting it to
    /// the bare name is unambiguous: returns (type index, number of segments used).
    fn type_prefix(&self, segs: &[String]) -> Option<(usize, usize)> {
        if segs.len() < 2 || self.shadows.contains(&segs[0]) || !is_lower(&segs[0]) {
            return None;
        }
        let k = segs.iter().position(|s| is_upper(s))?;
        if k == 0 || !segs[..k].iter().all(|s| is_lower(s)) {
            return None;
        }
        let pkg = &segs[..k];
        let name = &segs[k];
        let ti = self
            .types
            .iter()
            .position(|t| &t.name == name && t.package == pkg)?;
        // The bare name must mean this type from here.
        let same_name: Vec<&TypeInfo> = self.types.iter().filter(|t| &t.name == name).collect();
        if same_name.len() > 1 && self.types[ti].package != self.package {
            return None;
        }
        Some((ti, k + 1))
    }

    fn module_import_path(&self, ti: usize) -> Vec<String> {
        let t = &self.types[ti];
        let m = &self.modules[t.module_index];
        let stem = m
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&t.name)
            .to_string();
        let mut p = m.dir.clone();
        p.push(stem);
        p
    }

    fn expr(&mut self, e: &mut Expr) {
        if let Some(segs) = chain(e) {
            if let Some((ti, used)) = self.type_prefix(&segs) {
                let mut rest = vec![self.types[ti].name.clone()];
                rest.extend_from_slice(&segs[used..]);
                *e = rebuild(&rest);
                if self.types[ti].package != self.package {
                    let p = self.module_import_path(ti);
                    self.imports.insert(p);
                }
            }
            return;
        }
        match e {
            Expr::Field(r, _) | Expr::SafeField(r, _) => self.expr(r),
            Expr::Index(a, b) | Expr::NullCoalesce(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Call(t, args) => {
                self.expr(t);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::New(_, args) => {
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Unary { expr, .. } => self.expr(expr),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Ternary { cond, then, els } => {
                self.expr(cond);
                self.expr(then);
                self.expr(els);
            }
            Expr::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Expr::ArrayLit(v) => {
                for x in v {
                    self.expr(x);
                }
            }
            Expr::MapLit(v) => {
                for (k, x) in v {
                    self.expr(k);
                    self.expr(x);
                }
            }
            Expr::ObjectLit(v) => {
                for (_, x) in v {
                    self.expr(x);
                }
            }
            Expr::Comprehension {
                iter, guard, body, ..
            } => {
                self.iter(iter);
                if let Some(g) = guard {
                    self.expr(g);
                }
                match body {
                    ComprBody::Value(v) => self.expr(v),
                    ComprBody::KeyValue(k, v) => {
                        self.expr(k);
                        self.expr(v);
                    }
                }
            }
            Expr::Lambda { body, .. } => match &mut **body {
                LambdaBody::Expr(x) => self.expr(x),
                LambdaBody::Block(b) => self.stmts(b),
            },
            Expr::Cast { expr, .. }
            | Expr::TypeCheck { expr, .. }
            | Expr::Is { expr, .. }
            | Expr::Paren(expr)
            | Expr::Untyped(expr)
            | Expr::Meta(_, expr) => self.expr(expr),
            Expr::Switch {
                subject,
                cases,
                default,
            } => {
                self.expr(subject);
                self.cases(cases);
                if let Some(d) = default {
                    self.stmts(d);
                }
            }
            Expr::If { cond, then, els } => {
                self.expr(cond);
                self.expr(then);
                if let Some(x) = els {
                    self.expr(x);
                }
            }
            Expr::Block(b) => self.stmts(b),
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str { .. }
            | Expr::Bool(_)
            | Expr::Null
            | Expr::This
            | Expr::Super
            | Expr::Ident(_)
            | Expr::Regex { .. } => {}
        }
    }

    fn iter(&mut self, it: &mut Iterable) {
        match it {
            Iterable::Range(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Iterable::Coll(e) => self.expr(e),
        }
    }

    fn cases(&mut self, cases: &mut [Case]) {
        for c in cases {
            for p in &mut c.patterns {
                self.expr(p);
            }
            self.stmts(&mut c.body);
        }
    }

    fn stmts(&mut self, v: &mut [Stmt]) {
        for s in v {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Var { init, .. } => {
                if let Some(e) = init {
                    self.expr(e);
                }
            }
            Stmt::Expr(e, _) | Stmt::Throw(e, _) => self.expr(e),
            Stmt::Return(e, _) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::If { cond, then, els, .. } => {
                self.expr(cond);
                self.stmt(then);
                if let Some(e) = els {
                    self.stmt(e);
                }
            }
            Stmt::For { iter, body, .. } => {
                self.iter(iter);
                self.stmt(body);
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.stmt(body);
            }
            Stmt::Switch {
                subject,
                cases,
                default,
                ..
            } => {
                self.expr(subject);
                self.cases(cases);
                if let Some(d) = default {
                    self.stmts(d);
                }
            }
            Stmt::Try { body, catches, .. } => {
                self.stmt(body);
                for c in catches {
                    self.stmts(&mut c.body);
                }
            }
            Stmt::Block(b) => self.stmts(b),
            Stmt::Break | Stmt::Continue | Stmt::Verbatim { .. } => {}
        }
    }

    /// One function: its parameters and every name it declares shadow package roots.
    fn function(&mut self, f: &mut Function, members: &BTreeSet<String>) {
        let mut shadows = members.clone();
        for p in &f.params {
            shadows.insert(p.name.clone());
        }
        if let Some(body) = &f.body {
            locals_in_stmts(body, &mut shadows);
        }
        for p in &mut f.params {
            if let Some(d) = &mut p.default {
                let saved = std::mem::replace(&mut self.shadows, shadows.clone());
                self.expr(d);
                self.shadows = saved;
            }
        }
        if let Some(body) = &mut f.body {
            let saved = std::mem::replace(&mut self.shadows, shadows);
            self.stmts(body);
            self.shadows = saved;
        }
    }
}

impl Program {
    /// See the module documentation. Runs between `index_types` and `resolve_imports`.
    pub(super) fn qualify_type_paths(&mut self) {
        for mi in 0..self.modules.len() {
            // Work on a detached copy of the file so the context can borrow the rest.
            let mut file = std::mem::replace(
                &mut self.modules[mi].file,
                File {
                    package: Vec::new(),
                    imports: Vec::new(),
                    usings: Vec::new(),
                    decls: Vec::new(),
                    meta: Vec::new(),
                },
            );
            let mut cx = Ctx {
                types: &self.types,
                modules: &self.modules,
                package: self.modules[mi].package.clone(),
                shadows: BTreeSet::new(),
                imports: BTreeSet::new(),
            };
            for d in &mut file.decls {
                match d {
                    Decl::Class(c) => {
                        let mut members: BTreeSet<String> =
                            c.fields.iter().map(|f| f.name.clone()).collect();
                        for m in &c.methods {
                            if let Some(n) = &m.name {
                                members.insert(n.clone());
                            }
                        }
                        for f in &mut c.fields {
                            if let Some(e) = &mut f.init {
                                cx.shadows = members.clone();
                                cx.expr(e);
                            }
                        }
                        for m in &mut c.methods {
                            cx.function(m, &members);
                        }
                        if let Some(ctor) = &mut c.ctor {
                            cx.function(ctor, &members);
                        }
                    }
                    Decl::Function(f) => cx.function(f, &BTreeSet::new()),
                    Decl::Global(g) => {
                        if let Some(e) = &mut g.init {
                            cx.shadows = BTreeSet::new();
                            cx.expr(e);
                        }
                    }
                    Decl::Enum(e) => {
                        for v in &mut e.variants {
                            if let Some(x) = &mut v.value {
                                cx.shadows = BTreeSet::new();
                                cx.expr(x);
                            }
                        }
                    }
                    Decl::Interface(_) | Decl::Typedef(_) | Decl::Unsupported { .. } => {}
                }
            }
            let added: Vec<Vec<String>> = cx.imports.into_iter().collect();
            for path in added {
                if !file.imports.iter().any(|i| i.path == path && !i.wildcard) {
                    file.imports.push(Import {
                        path,
                        wildcard: false,
                        alias: None,
                    });
                }
            }
            self.modules[mi].file = file;
        }
    }
}
