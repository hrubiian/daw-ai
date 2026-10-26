//! Mini-notation — a terse Strudel/TidalCycles-style pattern language.
//!
//! A string describes one cycle of music; whitespace-separated terms divide the
//! cycle's time equally. Supported syntax:
//!
//! | Syntax        | Meaning                                              |
//! |---------------|------------------------------------------------------|
//! | `bd sd hh`    | sequence — three equal events across the cycle       |
//! | `~`           | rest                                                 |
//! | `[a b]`       | sub-group — plays in one term's time slot            |
//! | `a , b`       | stack — parallel layers (polyphony)                  |
//! | `a*3`         | repeat `a` three times within its slot               |
//! | `a!3`         | replicate — expands to three separate terms          |
//! | `a@3 b`       | weight — `a` takes 3 shares of time, `b` takes 1     |
//! | `bd(3,8)`     | Euclidean rhythm — 3 pulses across 8 steps           |
//! | `bd(3,8,2)`   | Euclidean with rotation 2                            |
//!
//! Token values resolve to MIDI pitches: drum names (`bd sd hh oh cp …`),
//! note names (`c3 g#4 eb2`, default octave 4), or raw MIDI integers (`60`).
//!
//! Cross-cycle operators (`<a b>` alternation, `a/2` slow) are not yet supported.

use crate::generative::euclidean;
use crate::project::{Note, PianoRollPattern};

/// A timed event within one cycle; `begin`/`end` are fractions in `[0, 1)`.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniEvent {
    pub begin: f64,
    pub end:   f64,
    pub value: String,
}

// ── AST ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Atom(String),
    Seq(Vec<Term>),
    Stack(Vec<Node>),
    Repeat(Box<Node>, u32),
    Euclid(Box<Node>, u32, u32, u32),
}

#[derive(Debug, Clone, PartialEq)]
struct Term { node: Node, weight: f64 }

// ── Tokenizer ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok { Word(String), LB, RB, LP, RP, Comma, Star, Bang, At }

fn tokenize(input: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut cur = String::new();
    fn flush(cur: &mut String, out: &mut Vec<Tok>) {
        if !cur.is_empty() { out.push(Tok::Word(std::mem::take(cur))); }
    }
    for c in input.chars() {
        match c {
            ' ' | '\t' | '\n' | '\r' => flush(&mut cur, &mut out),
            '[' => { flush(&mut cur, &mut out); out.push(Tok::LB); }
            ']' => { flush(&mut cur, &mut out); out.push(Tok::RB); }
            '(' => { flush(&mut cur, &mut out); out.push(Tok::LP); }
            ')' => { flush(&mut cur, &mut out); out.push(Tok::RP); }
            ',' => { flush(&mut cur, &mut out); out.push(Tok::Comma); }
            '*' => { flush(&mut cur, &mut out); out.push(Tok::Star); }
            '!' => { flush(&mut cur, &mut out); out.push(Tok::Bang); }
            '@' => { flush(&mut cur, &mut out); out.push(Tok::At); }
            _   => cur.push(c),
        }
    }
    flush(&mut cur, &mut out);
    out
}

// ── Parser (recursive descent) ────────────────────────────────────────────────

struct Parser { toks: Vec<Tok>, pos: usize }

impl Parser {
    fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    fn bump(&mut self) -> Option<Tok> { let t = self.toks.get(self.pos).cloned(); self.pos += 1; t }

    fn parse_stack(&mut self) -> Result<Node, String> {
        let mut seqs = vec![self.parse_seq()?];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.bump();
            seqs.push(self.parse_seq()?);
        }
        Ok(if seqs.len() == 1 { seqs.pop().unwrap() } else { Node::Stack(seqs) })
    }

    fn parse_seq(&mut self) -> Result<Node, String> {
        let mut terms = Vec::new();
        loop {
            match self.peek() {
                None | Some(Tok::RB) | Some(Tok::RP) | Some(Tok::Comma) => break,
                _ => terms.append(&mut self.parse_term()?),
            }
        }
        Ok(Node::Seq(terms))
    }

    fn parse_term(&mut self) -> Result<Vec<Term>, String> {
        let mut node = self.parse_primary()?;
        let mut weight = 1.0;
        let mut replicate = 1u32;
        loop {
            match self.peek() {
                Some(Tok::Star) => { self.bump(); node = Node::Repeat(Box::new(node), self.num()?.max(1)); }
                Some(Tok::Bang) => { self.bump(); replicate = self.num()?.max(1); }
                Some(Tok::At)   => { self.bump(); weight = (self.num()? as f64).max(0.001); }
                Some(Tok::LP)   => {
                    self.bump();
                    let p = self.num()?;
                    self.expect(Tok::Comma)?;
                    let s = self.num()?;
                    let r = if matches!(self.peek(), Some(Tok::Comma)) { self.bump(); self.num()? } else { 0 };
                    self.expect(Tok::RP)?;
                    node = Node::Euclid(Box::new(node), p, s, r);
                }
                _ => break,
            }
        }
        let term = Term { node, weight };
        Ok(if replicate <= 1 {
            vec![term]
        } else {
            (0..replicate).map(|_| Term { node: term.node.clone(), weight: term.weight }).collect()
        })
    }

    fn parse_primary(&mut self) -> Result<Node, String> {
        match self.bump() {
            Some(Tok::LB)        => { let n = self.parse_stack()?; self.expect(Tok::RB)?; Ok(n) }
            Some(Tok::Word(w))   => Ok(Node::Atom(w)),
            other                => Err(format!("unexpected token {other:?}")),
        }
    }

    fn num(&mut self) -> Result<u32, String> {
        match self.bump() {
            Some(Tok::Word(w)) => w.parse::<u32>().map_err(|_| format!("expected a number, got `{w}`")),
            other              => Err(format!("expected a number, got {other:?}")),
        }
    }

    fn expect(&mut self, t: Tok) -> Result<(), String> {
        if self.peek() == Some(&t) { self.bump(); Ok(()) } else { Err(format!("expected {t:?}")) }
    }
}

/// Parse mini-notation into a flat list of timed events over one cycle.
pub fn parse(input: &str) -> Result<Vec<MiniEvent>, String> {
    let mut p = Parser { toks: tokenize(input), pos: 0 };
    let node = p.parse_stack()?;
    if p.pos != p.toks.len() {
        return Err(format!("unexpected trailing token {:?}", p.toks.get(p.pos)));
    }
    let mut out = Vec::new();
    flatten(&node, 0.0, 1.0, &mut out);
    out.sort_by(|a, b| a.begin.partial_cmp(&b.begin).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

fn flatten(node: &Node, begin: f64, end: f64, out: &mut Vec<MiniEvent>) {
    match node {
        Node::Atom(v) => {
            if v != "~" && !v.is_empty() {
                out.push(MiniEvent { begin, end, value: v.clone() });
            }
        }
        Node::Seq(terms) => {
            let total: f64 = terms.iter().map(|t| t.weight).sum();
            if total <= 0.0 { return; }
            let span = end - begin;
            let mut cur = begin;
            for t in terms {
                let w = span * t.weight / total;
                flatten(&t.node, cur, cur + w, out);
                cur += w;
            }
        }
        Node::Stack(nodes) => {
            for n in nodes { flatten(n, begin, end, out); }
        }
        Node::Repeat(inner, n) => {
            let n = (*n).max(1);
            let span = (end - begin) / n as f64;
            for i in 0..n {
                let b = begin + span * i as f64;
                flatten(inner, b, b + span, out);
            }
        }
        Node::Euclid(inner, p, s, r) => {
            let pat = euclidean(*p as usize, *s as usize, *r as usize);
            let steps = pat.len().max(1);
            let span = (end - begin) / steps as f64;
            for (i, &hit) in pat.iter().enumerate() {
                if hit == 1 {
                    let b = begin + span * i as f64;
                    flatten(inner, b, b + span, out);
                }
            }
        }
    }
}

// ── Token → MIDI pitch ────────────────────────────────────────────────────────

/// Resolve a token to a MIDI pitch: drum name, then note name, then raw integer.
/// Returns `None` for rests/unknown tokens.
pub fn resolve_token(tok: &str) -> Option<u8> {
    let t = tok.trim();
    if t.is_empty() || t == "~" { return None; }
    drum_pitch(t)
        .or_else(|| t.parse::<i32>().ok().filter(|n| (0..=127).contains(n)).map(|n| n as u8))
        .or_else(|| note_pitch(t))
}

fn drum_pitch(s: &str) -> Option<u8> {
    Some(match s {
        "bd" | "kick"        => 36,
        "sd" | "snare"       => 38,
        "rs" | "rim"         => 37,
        "cp" | "clap"        => 39,
        "lt"                 => 45,
        "mt"                 => 47,
        "ht"                 => 50,
        "hh" | "ch" | "hat"  => 42,
        "oh"                 => 46,
        "cr" | "crash"       => 49,
        "rd" | "ride"        => 51,
        "cb" | "cowbell"     => 56,
        "tb" | "tamb"        => 54,
        _ => return None,
    })
}

fn note_pitch(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    let base: i32 = match b.first()?.to_ascii_lowercase() {
        b'c' => 0, b'd' => 2, b'e' => 4, b'f' => 5, b'g' => 7, b'a' => 9, b'b' => 11,
        _ => return None,
    };
    let mut i = 1;
    let mut semi = base;
    match b.get(i) {
        Some(b'#') => { semi += 1; i += 1; }
        Some(b'b') => { semi -= 1; i += 1; }
        _ => {}
    }
    let oct: i32 = if i < b.len() { s[i..].parse().ok()? } else { 4 };
    let midi = (oct + 1) * 12 + semi;
    (0..=127).contains(&midi).then_some(midi as u8)
}

// ── High-level compile ────────────────────────────────────────────────────────

/// Turn events into piano-roll notes spanning `total_ticks`.
pub fn to_notes(events: &[MiniEvent], total_ticks: u32, vel: u8) -> Vec<Note> {
    events.iter().filter_map(|e| {
        let pitch = resolve_token(&e.value)?;
        let tick = (e.begin * total_ticks as f64).round() as u32;
        let duration = (((e.end - e.begin) * total_ticks as f64).round() as u32).max(1);
        Some(Note { pitch, tick, duration, vel })
    }).collect()
}

/// Compile a mini-notation string into a `bars`-long piano-roll pattern.
pub fn compile(input: &str, id: &str, name: &str, bars: u8, ticks_per_bar: u32, vel: u8)
    -> Result<PianoRollPattern, String>
{
    let events = parse(input)?;
    let total = ticks_per_bar * bars.max(1) as u32;
    Ok(PianoRollPattern {
        id: id.into(),
        name: name.into(),
        bars: bars.max(1),
        ticks_per_bar,
        notes: to_notes(&events, total, vel), legato: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begins(s: &str) -> Vec<f64> { parse(s).unwrap().iter().map(|e| e.begin).collect() }

    #[test]
    fn sequences_and_rests() {
        assert_eq!(begins("bd sd"), vec![0.0, 0.5]);
        assert_eq!(begins("a b c d"), vec![0.0, 0.25, 0.5, 0.75]);
        // rests take time but emit no event
        assert_eq!(begins("bd ~ sd ~"), vec![0.0, 0.5]);
    }

    #[test]
    fn groups_repeat_replicate_weight() {
        // sub-group shares one slot
        assert_eq!(begins("bd [sd sd]"), vec![0.0, 0.5, 0.75]);
        // *n subdivides the slot
        assert_eq!(begins("bd*2"), vec![0.0, 0.5]);
        // !n replicates into separate slots
        assert_eq!(begins("bd!3"), vec![0.0, 1.0/3.0, 2.0/3.0]);
        // weight: a takes 3/4, b takes 1/4
        assert_eq!(begins("a@3 b"), vec![0.0, 0.75]);
    }

    #[test]
    fn stack_and_euclid() {
        // stack overlays layers (4 + 8 = 12 events)
        assert_eq!(parse("bd*4, hh*8").unwrap().len(), 12);
        // euclid places p pulses
        assert_eq!(parse("bd(3,8)").unwrap().len(), 3);
        assert_eq!(parse("hh(5,16)").unwrap().len(), 5);
    }

    #[test]
    fn compiles_to_notes() {
        let p = compile("bd sd hh hh", "p1", "beat", 1, 480, 100).unwrap();
        assert_eq!(p.notes.len(), 4);
        assert_eq!(p.notes[0].pitch, 36); // bd
        assert_eq!(p.notes[1].pitch, 38); // sd
        assert_eq!(p.notes[0].tick, 0);
        assert_eq!(p.notes[1].tick, 120); // 0.25 * 480
        // note names
        let m = compile("c4 e4 g4", "p2", "chord", 1, 480, 90).unwrap();
        assert_eq!(m.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
    }

    #[test]
    fn token_resolution() {
        assert_eq!(resolve_token("bd"), Some(36));
        assert_eq!(resolve_token("c4"), Some(60));
        assert_eq!(resolve_token("g#3"), Some(56));
        assert_eq!(resolve_token("eb4"), Some(63));
        assert_eq!(resolve_token("60"), Some(60));
        assert_eq!(resolve_token("~"), None);
        assert_eq!(resolve_token("zzz"), None);
    }

    #[test]
    fn bad_input_errors_not_panics() {
        assert!(parse("bd ]").is_err());
        assert!(parse("bd(3").is_err());
    }
}
