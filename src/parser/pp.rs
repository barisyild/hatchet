//! Conditional compilation around declarations and class members.
//!
//! Inside a function body a Haxe `#if` is handed to the C++ preprocessor (see
//! `parse_stmt`). Around a class member or a top-level declaration that cannot work:
//! the member is split between a header declaration and a `.cpp` definition, and an
//! import or a whole type has no single place to wrap. There the condition is decided
//! here, as Haxe decides it, against the flags given with `-D NAME`: a flag is true
//! when defined, and `!`, `&&`, `||` and parentheses combine flags. The branch that
//! holds is parsed; the others are skipped token by token, nested directives included.
//! A condition outside that grammar (a version comparison, a value) is an error.

use super::*;
use crate::lexer::PpKind;
use std::cell::RefCell;
use std::collections::BTreeSet;

thread_local! {
    static DEFINES: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

/// The flags `#if` tests true against, at member and declaration level (`-D NAME`).
pub fn set_defines<I: IntoIterator<Item = String>>(names: I) {
    DEFINES.with(|d| *d.borrow_mut() = names.into_iter().collect());
}

fn defined(name: &str) -> bool {
    DEFINES.with(|d| d.borrow().contains(name))
}

struct Cond<'c> {
    s: &'c [u8],
    i: usize,
}

impl Cond<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn or(&mut self) -> Option<bool> {
        let mut v = self.and()?;
        while self.eat("||") {
            let r = self.and()?;
            v = v || r;
        }
        Some(v)
    }
    fn and(&mut self) -> Option<bool> {
        let mut v = self.unary()?;
        while self.eat("&&") {
            let r = self.unary()?;
            v = v && r;
        }
        Some(v)
    }
    fn unary(&mut self) -> Option<bool> {
        if self.eat("!") {
            return Some(!self.unary()?);
        }
        if self.eat("(") {
            let v = self.or()?;
            return if self.eat(")") { Some(v) } else { None };
        }
        self.ws();
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric()
                || self.s[self.i] == b'_'
                || self.s[self.i] == b'.')
        {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        Some(defined(std::str::from_utf8(&self.s[start..self.i]).ok()?))
    }
}

/// Evaluate a directive condition over `-D` flags; `None` if it is outside the grammar.
pub(super) fn evaluate(cond: &str) -> Option<bool> {
    let mut c = Cond {
        s: cond.as_bytes(),
        i: 0,
    };
    let v = c.or()?;
    c.ws();
    if c.i == c.s.len() {
        Some(v)
    } else {
        None
    }
}

impl Parser<'_> {
    fn pp_eval(&self, cond: &str) -> PResult<bool> {
        evaluate(cond).ok_or_else(|| {
            self.err(&format!(
                "conditional compilation around a declaration or member takes flags combined \
                 with `!`, `&&`, `||` and parentheses (decided with `-D NAME`); `#if {cond}` is \
                 outside that"
            ))
        })
    }

    /// Skip a branch that does not hold: up to the `#elseif` that does, the `#else`, or
    /// the `#end` of this directive, whichever comes first at this nesting level.
    fn pp_skip_inactive(&mut self) -> PResult<()> {
        let mut depth = 0usize;
        loop {
            if self.at_eof() {
                return Err(self.err("unterminated `#if` (no `#end`)"));
            }
            match self.peek().clone() {
                TokKind::Pp(PpKind::If, _) => depth += 1,
                TokKind::Pp(PpKind::End, _) if depth > 0 => depth -= 1,
                TokKind::Pp(PpKind::End, _) => {
                    self.bump();
                    return Ok(());
                }
                TokKind::Pp(PpKind::ElseIf, cond) if depth == 0 => {
                    self.bump();
                    if self.pp_eval(&cond)? {
                        return Ok(());
                    }
                    continue;
                }
                TokKind::Pp(PpKind::Else, _) if depth == 0 => {
                    self.bump();
                    return Ok(());
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// After the branch that held: skip every remaining branch to this directive's `#end`.
    fn pp_skip_to_end(&mut self) -> PResult<()> {
        let mut depth = 0usize;
        loop {
            if self.at_eof() {
                return Err(self.err("unterminated `#if` (no `#end`)"));
            }
            match self.peek() {
                TokKind::Pp(PpKind::If, _) => depth += 1,
                TokKind::Pp(PpKind::End, _) if depth > 0 => depth -= 1,
                TokKind::Pp(PpKind::End, _) => {
                    self.bump();
                    return Ok(());
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// A directive at declaration or member level, decided here. Returns whether one was
    /// consumed, so the caller's loop moves on to whatever the holding branch contains.
    pub(super) fn outer_directive(&mut self) -> PResult<bool> {
        let TokKind::Pp(kind, cond) = self.peek().clone() else {
            return Ok(false);
        };
        self.bump();
        match kind {
            PpKind::If => {
                if !self.pp_eval(&cond)? {
                    self.pp_skip_inactive()?;
                }
            }
            // Reached at the end of the branch that held.
            PpKind::ElseIf | PpKind::Else => self.pp_skip_to_end()?,
            PpKind::End => {}
        }
        Ok(true)
    }
}
