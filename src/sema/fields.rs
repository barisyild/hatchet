//! Field types from initialisers, and integer constants folded.
//!
//! Haxe infers a field's type from its initialiser: `static var count = 0;` is an
//! `Int`, `static final TABLE = [1, 2, 3];` an `Array<Int>`, `static inline var MASK =
//! SIZE - 1;` an `Int`. Hatchet read the type only from an annotation, so an
//! unannotated field became `void*` and every local initialised from it lost its
//! declaration. This pass, run after imports are resolved, gives each unannotated field
//! the type its initialiser has, by the rules Haxe uses for the forms it recognises:
//! literals, arithmetic, comparisons, references to other fields (in this class or,
//! as `Type.NAME`, in another), array literals and comprehensions, `new`, and typed
//! casts. It repeats until nothing changes, so a field defined from another field
//! resolves whatever their order. A field it cannot type keeps no annotation, and the
//! downstream behaviour is exactly what it was.
//!
//! It then folds integer constants. A static `Int` whose initialiser is built only
//! from literals and other `inline`/`final` integer statics is evaluated here, with
//! Haxe's 32-bit semantics (wrapping arithmetic, shift counts taken modulo 32, `>>>`
//! unsigned), and its initialiser replaced by the literal. That makes it a literal
//! initialiser downstream: a plain class static rather than a Meyers accessor (a
//! function call on every read), and, for an `inline`/`final` field, a C++ integral
//! constant the compiler can fold (see `codegen::is_const_static`).

use super::Program;
use crate::ast::*;

fn named(name: &str, params: Vec<Type>) -> Type {
    Type::Named {
        path: vec![name.to_string()],
        params,
        optional: false,
        line: 0,
    }
}

fn is_named(t: &Type, name: &str) -> bool {
    matches!(t, Type::Named { path, params, .. } if params.is_empty() && path.last().is_some_and(|n| n == name))
}

/// Where a field lives: (module, decl, field) indices.
type FieldRef = (usize, usize, usize);

impl Program {
    fn class_decl(&self, mi: usize, name: &str) -> Option<(usize, &Class)> {
        self.modules[mi]
            .file
            .decls
            .iter()
            .enumerate()
            .find_map(|(di, d)| match d {
                Decl::Class(c) if c.name == *name => Some((di, &**c)),
                _ => None,
            })
    }

    /// The field an expression names: `NAME` in the same class, or `Type.NAME`.
    fn named_field(&self, e: &Expr, here: (usize, usize)) -> Option<FieldRef> {
        match e {
            Expr::Ident(n) => {
                let Decl::Class(c) = &self.modules[here.0].file.decls[here.1] else {
                    return None;
                };
                let fi = c.fields.iter().position(|f| f.name == *n)?;
                Some((here.0, here.1, fi))
            }
            Expr::Field(recv, n) => {
                let Expr::Ident(tname) = &**recv else {
                    return None;
                };
                let info = self.resolve_type(std::slice::from_ref(tname), here.0)?;
                let mi = info.module_index;
                let (di, c) = self.class_decl(mi, &info.name)?;
                let fi = c.fields.iter().position(|f| f.name == *n && f.is_static)?;
                Some((mi, di, fi))
            }
            _ => None,
        }
    }

    fn field_at(&self, r: FieldRef) -> &Field {
        let Decl::Class(c) = &self.modules[r.0].file.decls[r.1] else {
            unreachable!("a FieldRef always points into a class");
        };
        &c.fields[r.2]
    }

    fn infer_expr(&self, e: &Expr, here: (usize, usize)) -> Option<Type> {
        match e {
            Expr::Int(_) => Some(named("Int", vec![])),
            Expr::Float(_) => Some(named("Float", vec![])),
            Expr::Bool(_) => Some(named("Bool", vec![])),
            Expr::Str { .. } => Some(named("String", vec![])),
            Expr::Paren(x) => self.infer_expr(x, here),
            Expr::Unary { op, expr, .. } => match op {
                UnOp::Not => Some(named("Bool", vec![])),
                UnOp::Neg | UnOp::BitNot | UnOp::Incr | UnOp::Decr => self.infer_expr(expr, here),
            },
            Expr::Binary { op, lhs, rhs } => {
                use BinOp::*;
                match op {
                    Eq | Ne | Lt | Gt | Le | Ge | And | Or => Some(named("Bool", vec![])),
                    BitAnd | BitOr | BitXor | Shl | Shr | UShr => Some(named("Int", vec![])),
                    Div => Some(named("Float", vec![])),
                    Add | Sub | Mul | Mod => {
                        let l = self.infer_expr(lhs, here)?;
                        let r = self.infer_expr(rhs, here)?;
                        if matches!(op, Add) && (is_named(&l, "String") || is_named(&r, "String")) {
                            Some(named("String", vec![]))
                        } else if is_named(&l, "Int") && is_named(&r, "Int") {
                            Some(named("Int", vec![]))
                        } else if (is_named(&l, "Float") || is_named(&l, "Int"))
                            && (is_named(&r, "Float") || is_named(&r, "Int"))
                        {
                            Some(named("Float", vec![]))
                        } else {
                            None
                        }
                    }
                }
            }
            Expr::Ternary { then, els, .. } => self
                .infer_expr(then, here)
                .or_else(|| self.infer_expr(els, here)),
            Expr::Ident(_) | Expr::Field(..) => {
                let r = self.named_field(e, here)?;
                self.field_at(r).ty.clone()
            }
            Expr::ArrayLit(items) => {
                let first = items.first()?;
                let t = self.infer_expr(first, here)?;
                Some(named("Array", vec![t]))
            }
            Expr::Comprehension {
                body: ComprBody::Value(v),
                ..
            } => {
                let t = self.infer_expr(v, here)?;
                Some(named("Array", vec![t]))
            }
            Expr::New(t, _) => Some(t.clone()),
            Expr::Cast { ty: Some(t), .. } => Some(t.clone()),
            _ => None,
        }
    }

    /// Haxe `Int` evaluation of a constant integer expression, or `None`.
    fn fold_int(&self, e: &Expr, here: (usize, usize), depth: u32) -> Option<i32> {
        if depth > 32 {
            return None;
        }
        match e {
            Expr::Int(s) => {
                let t = s.trim();
                if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    let v = u64::from_str_radix(h, 16).ok()?;
                    if v > u32::MAX as u64 {
                        return None;
                    }
                    Some(v as u32 as i32)
                } else {
                    let v: i64 = t.parse().ok()?;
                    if v > i32::MAX as i64 {
                        return None;
                    }
                    Some(v as i32)
                }
            }
            Expr::Paren(x) => self.fold_int(x, here, depth + 1),
            Expr::Unary {
                op: UnOp::Neg,
                expr,
                prefix: true,
            } => Some(self.fold_int(expr, here, depth + 1)?.wrapping_neg()),
            Expr::Unary {
                op: UnOp::BitNot,
                expr,
                prefix: true,
            } => Some(!self.fold_int(expr, here, depth + 1)?),
            Expr::Binary { op, lhs, rhs } => {
                use BinOp::*;
                let a = self.fold_int(lhs, here, depth + 1)?;
                let b = self.fold_int(rhs, here, depth + 1)?;
                Some(match op {
                    Add => a.wrapping_add(b),
                    Sub => a.wrapping_sub(b),
                    Mul => a.wrapping_mul(b),
                    Mod if b != 0 => a.wrapping_rem(b),
                    BitAnd => a & b,
                    BitOr => a | b,
                    BitXor => a ^ b,
                    Shl => a.wrapping_shl((b & 31) as u32),
                    Shr => a.wrapping_shr((b & 31) as u32),
                    UShr => ((a as u32) >> ((b & 31) as u32)) as i32,
                    _ => return None,
                })
            }
            Expr::Ident(_) | Expr::Field(..) => {
                let r = self.named_field(e, here)?;
                let f = self.field_at(r);
                if !(f.is_static && (f.is_inline || f.is_final)) {
                    return None;
                }
                if let Some(t) = &f.ty {
                    if !is_named(t, "Int") {
                        return None;
                    }
                }
                let init = f.init.as_ref()?;
                self.fold_int(init, (r.0, r.1), depth + 1)
            }
            _ => None,
        }
    }

    /// See the module documentation. Runs after `resolve_imports`.
    pub(super) fn infer_field_types(&mut self) {
        // Types, to a fixed point.
        for _ in 0..16 {
            let mut updates: Vec<(FieldRef, Type)> = Vec::new();
            for mi in 0..self.modules.len() {
                for (di, d) in self.modules[mi].file.decls.iter().enumerate() {
                    let Decl::Class(c) = d else { continue };
                    for (fi, f) in c.fields.iter().enumerate() {
                        if f.ty.is_some() {
                            continue;
                        }
                        let Some(init) = &f.init else { continue };
                        if let Some(t) = self.infer_expr(init, (mi, di)) {
                            updates.push(((mi, di, fi), t));
                        }
                    }
                }
            }
            if updates.is_empty() {
                break;
            }
            for ((mi, di, fi), t) in updates {
                if let Decl::Class(c) = &mut self.modules[mi].file.decls[di] {
                    c.fields[fi].ty = Some(t);
                }
            }
        }
        // Integer constants, folded to literals.
        let mut folds: Vec<(FieldRef, i32)> = Vec::new();
        for mi in 0..self.modules.len() {
            for (di, d) in self.modules[mi].file.decls.iter().enumerate() {
                let Decl::Class(c) = d else { continue };
                for (fi, f) in c.fields.iter().enumerate() {
                    if !f.is_static || !f.ty.as_ref().is_some_and(|t| is_named(t, "Int")) {
                        continue;
                    }
                    let Some(init) = &f.init else { continue };
                    if matches!(init, Expr::Int(_)) {
                        continue;
                    }
                    if let Some(v) = self.fold_int(init, (mi, di), 0) {
                        folds.push(((mi, di, fi), v));
                    }
                }
            }
        }
        for ((mi, di, fi), v) in folds {
            if let Decl::Class(c) = &mut self.modules[mi].file.decls[di] {
                // The one value C++ cannot spell as a negated literal: `-2147483648` would
                // negate a `long`. Its hex pattern goes through `int_lit`'s cast instead.
                let lit = if v == i32::MIN {
                    "0x80000000".to_string()
                } else {
                    v.to_string()
                };
                c.fields[fi].init = Some(Expr::Int(lit));
            }
        }
    }
}
