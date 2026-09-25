//! Haxe names that are C++ keywords.
//!
//! `register`, `char`, `union`, `delete`, `signed` and the rest are ordinary identifiers in
//! Haxe and reserved words in C++, so a method `Overlays.register()` came out as
//! `static void register();`, which does not compile. Every name the program declares or
//! uses is taken from the syntax tree, so renaming in the tree, once, before anything else
//! runs, renames the declaration and every use together, across modules: a name that is a
//! C++ keyword gets a trailing underscore (`register_`). Only C++ keywords that Haxe does
//! not also reserve are listed; a Haxe keyword cannot be an identifier to begin with. Names
//! inside `__cpp__` strings and `@:native` metadata are hand-written C++ and left alone.

use super::Module;
use crate::ast::*;

const CPP_ONLY_KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "auto",
    "bitand",
    "bitor",
    "bool",
    "char",
    "char16_t",
    "char32_t",
    "compl",
    "const",
    "const_cast",
    "constexpr",
    "decltype",
    "delete",
    "double",
    "dynamic_cast",
    "explicit",
    "export",
    "float",
    "friend",
    "goto",
    "int",
    "long",
    "mutable",
    "namespace",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "protected",
    "register",
    "reinterpret_cast",
    "short",
    "signed",
    "sizeof",
    "static_assert",
    "static_cast",
    "struct",
    "template",
    "thread_local",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "virtual",
    "void",
    "volatile",
    "wchar_t",
    "xor",
    "xor_eq",
];

fn fix(name: &mut String) {
    if CPP_ONLY_KEYWORDS.contains(&name.as_str()) {
        name.push('_');
    }
}

fn expr(e: &mut Expr) {
    match e {
        Expr::Ident(n) => fix(n),
        // `Std.int(x)` and friends are Haxe built-ins that codegen recognises by name.
        Expr::Field(r, _) if matches!(&**r, Expr::Ident(t) if t == "Std") => {}
        Expr::Field(r, n) | Expr::SafeField(r, n) => {
            expr(r);
            fix(n);
        }
        Expr::Index(a, b) | Expr::NullCoalesce(a, b) => {
            expr(a);
            expr(b);
        }
        Expr::Call(t, args) => {
            expr(t);
            args.iter_mut().for_each(expr);
        }
        Expr::New(_, args) => args.iter_mut().for_each(expr),
        Expr::Unary { expr: x, .. } => expr(x),
        Expr::Binary { lhs, rhs, .. } => {
            expr(lhs);
            expr(rhs);
        }
        Expr::Ternary { cond, then, els } => {
            expr(cond);
            expr(then);
            expr(els);
        }
        Expr::Assign { target, value, .. } => {
            expr(target);
            expr(value);
        }
        Expr::ArrayLit(v) => v.iter_mut().for_each(expr),
        Expr::MapLit(v) => v.iter_mut().for_each(|(k, x)| {
            expr(k);
            expr(x);
        }),
        Expr::ObjectLit(v) => v.iter_mut().for_each(|(k, x)| {
            fix(k);
            expr(x);
        }),
        Expr::Comprehension {
            var,
            value_var,
            iter: it,
            guard,
            body,
        } => {
            fix(var);
            if let Some(v) = value_var {
                fix(v);
            }
            iter(it);
            if let Some(g) = guard {
                expr(g);
            }
            match body {
                ComprBody::Value(v) => expr(v),
                ComprBody::KeyValue(k, v) => {
                    expr(k);
                    expr(v);
                }
            }
        }
        Expr::Lambda { params, body, .. } => {
            params.iter_mut().for_each(param);
            match &mut **body {
                LambdaBody::Expr(x) => expr(x),
                LambdaBody::Block(b) => stmts(b),
            }
        }
        Expr::Cast { expr: x, .. }
        | Expr::TypeCheck { expr: x, .. }
        | Expr::Is { expr: x, .. }
        | Expr::Paren(x)
        | Expr::Untyped(x)
        | Expr::Meta(_, x) => expr(x),
        Expr::Switch {
            subject,
            cases: cs,
            default,
        } => {
            expr(subject);
            cases(cs);
            if let Some(d) = default {
                stmts(d);
            }
        }
        Expr::If { cond, then, els } => {
            expr(cond);
            expr(then);
            if let Some(x) = els {
                expr(x);
            }
        }
        Expr::Block(b) => stmts(b),
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str { .. }
        | Expr::Bool(_)
        | Expr::Null
        | Expr::This
        | Expr::Super
        | Expr::Regex { .. } => {}
    }
}

fn iter(it: &mut Iterable) {
    match it {
        Iterable::Range(a, b) => {
            expr(a);
            expr(b);
        }
        Iterable::Coll(e) => expr(e),
    }
}

fn cases(cs: &mut [Case]) {
    for c in cs {
        c.patterns.iter_mut().for_each(expr);
        stmts(&mut c.body);
    }
}

fn stmts(v: &mut [Stmt]) {
    v.iter_mut().for_each(stmt);
}

fn stmt(s: &mut Stmt) {
    match s {
        Stmt::Var { name, init, .. } => {
            fix(name);
            if let Some(e) = init {
                expr(e);
            }
        }
        Stmt::Expr(e, _) | Stmt::Throw(e, _) => expr(e),
        Stmt::Return(e, _) => {
            if let Some(e) = e {
                expr(e);
            }
        }
        Stmt::If {
            cond, then, els, ..
        } => {
            expr(cond);
            stmt(then);
            if let Some(e) = els {
                stmt(e);
            }
        }
        Stmt::For {
            var,
            value_var,
            iter: it,
            body,
            ..
        } => {
            fix(var);
            if let Some(v) = value_var {
                fix(v);
            }
            iter(it);
            stmt(body);
        }
        Stmt::While { cond, body, .. } => {
            expr(cond);
            stmt(body);
        }
        Stmt::Switch {
            subject,
            cases: cs,
            default,
            ..
        } => {
            expr(subject);
            cases(cs);
            if let Some(d) = default {
                stmts(d);
            }
        }
        Stmt::Try { body, catches, .. } => {
            stmt(body);
            for c in catches {
                fix(&mut c.name);
                stmts(&mut c.body);
            }
        }
        Stmt::Block(b) => stmts(b),
        Stmt::Break | Stmt::Continue | Stmt::Verbatim { .. } => {}
    }
}

fn param(p: &mut Param) {
    fix(&mut p.name);
    if let Some(d) = &mut p.default {
        expr(d);
    }
}

fn function(f: &mut Function) {
    if let Some(n) = &mut f.name {
        fix(n);
    }
    f.params.iter_mut().for_each(param);
    if let Some(b) = &mut f.body {
        stmts(b);
    }
}

fn field(f: &mut Field) {
    fix(&mut f.name);
    if let Some(e) = &mut f.init {
        expr(e);
    }
}

/// Rename C++ keywords throughout every module. Runs before types are indexed.
pub(super) fn rename(modules: &mut [Module]) {
    for m in modules {
        for d in &mut m.file.decls {
            match d {
                Decl::Class(c) => {
                    c.fields.iter_mut().for_each(field);
                    c.methods.iter_mut().for_each(function);
                    if let Some(ctor) = &mut c.ctor {
                        function(ctor);
                    }
                }
                Decl::Interface(i) => {
                    i.fields.iter_mut().for_each(field);
                    i.methods.iter_mut().for_each(function);
                }
                Decl::Function(f) => function(f),
                Decl::Global(g) => {
                    fix(&mut g.name);
                    if let Some(e) = &mut g.init {
                        expr(e);
                    }
                }
                Decl::Typedef(t) => {
                    if let TypedefTarget::Struct(fs) = &mut t.target {
                        fs.iter_mut().for_each(|f| fix(&mut f.name));
                    }
                }
                Decl::Enum(e) => {
                    for v in &mut e.variants {
                        v.params.iter_mut().for_each(param);
                        if let Some(x) = &mut v.value {
                            expr(x);
                        }
                    }
                }
                Decl::Unsupported { .. } => {}
            }
        }
    }
}
