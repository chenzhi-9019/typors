//! Deterministic generator for the mapping samples used by the H3 tests.
//!
//! No external RNG: a 64-bit LCG keeps the 100 samples identical on every run, so
//! a failure is reproducible from its sample index alone.

pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Lcg(seed | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u32) as usize]
    }
}

const WORDS: &[&str] = &[
    "文字",
    "编辑器",
    "混合",
    "渲染",
    "源码",
    "hello",
    "world",
    "spike",
    "Typora",
    "标点，",
    "句号。",
    "emoji😀",
    "漢字かな",
    "family👨‍👩‍👧‍👦",
    "accent é",
    "1.5ms",
    "「引用」",
];

const INFOS: &[&str] = &["rust", "text", "python", "", "ts"];

fn word(rng: &mut Lcg) -> String {
    rng.pick(WORDS).to_string()
}

fn sentence(rng: &mut Lcg, words: u32) -> String {
    let mut out = String::new();
    for _ in 0..words {
        out.push_str(&word(rng));
    }
    out
}

/// Inline constructs, including the nested emphasis and inline-code cases the
/// mapping has to survive.
fn inline(rng: &mut Lcg) -> String {
    match rng.below(12) {
        0 => {
            let n = 1 + rng.below(2);
            format!("**{}**", sentence(rng, n))
        }
        1 => {
            let n = 1 + rng.below(2);
            format!("*{}*", sentence(rng, n))
        }
        2 => format!("`{}`", word(rng)),
        3 => format!("**{} *{}* {}**", word(rng), word(rng), word(rng)),
        4 => format!("[{}](https://example.com/路径)", word(rng)),
        5 => format!("\\*{}", word(rng)),
        6 => format!("{}  \n{}", word(rng), word(rng)),
        7 => format!(
            "${}$",
            rng.pick(&["x^2", "\\frac{a}{b}", "\\sum_i i", "\\alpha + \\beta"])
        ),
        8 => format!("![图{}](assets/pic{}.png)", rng.below(3), rng.below(3)),
        9 => format!("$${}$$", rng.pick(&["x^2 + y^2", "\\int_0^1 x\\,dx"])),
        10 => format!("![图](https://example.com/{})", word(rng)),
        _ => {
            let n = 1 + rng.below(3);
            sentence(rng, n)
        }
    }
}

fn paragraph(rng: &mut Lcg) -> String {
    let mut out = String::new();
    for _ in 0..1 + rng.below(3) {
        out.push_str(&inline(rng));
        out.push(' ');
    }
    out.pop();
    out.push('\n');
    out
}

fn fenced_code(rng: &mut Lcg) -> String {
    let info = rng.pick(INFOS);
    let mut out = format!("```{info}\n");
    for line in 0..1 + rng.below(3) {
        out.push_str(&format!("let 值{line} = \"{}\";\n", word(rng)));
    }
    out.push_str("```\n");
    out
}

fn list(rng: &mut Lcg) -> String {
    let ordered = rng.below(2) == 0;
    let mut out = String::new();
    let items = 1 + rng.below(3);
    for i in 0..items {
        let marker = if ordered {
            format!("{}. ", i + 1)
        } else {
            "- ".to_string()
        };
        out.push_str(&marker);
        out.push_str(&inline(rng));
        out.push('\n');
        if rng.below(4) == 0 {
            let nested = if ordered { "   - " } else { "   1. " };
            out.push_str(nested);
            out.push_str(&sentence(rng, 2));
            out.push('\n');
        }
    }
    out
}

fn heading(rng: &mut Lcg) -> String {
    format!(
        "{} {}\n",
        "#".repeat(1 + rng.below(3) as usize),
        sentence(rng, 2)
    )
}

/// One mixed Markdown document, built from the six block categories plus blank
/// lines, CJK text, emoji, nested emphasis and out-of-scope syntax.
pub fn mixed_sample(index: usize) -> String {
    let mut rng = Lcg::new(0x9E37_79B9_7F4A_7C15 ^ (index as u64).wrapping_mul(2_654_435_761));
    let mut out = String::new();
    for _ in 0..3 + rng.below(6) {
        match rng.below(10) {
            0 | 1 => out.push_str(&heading(&mut rng)),
            2 | 3 | 4 => out.push_str(&paragraph(&mut rng)),
            5 | 6 => out.push_str(&fenced_code(&mut rng)),
            7 | 8 => out.push_str(&list(&mut rng)),
            _ => out.push_str(&match rng.below(5) {
                0 => "> 引用里的一段 **文字**\n".to_string(),
                1 => "| a | b |\n|---|---|\n| 1 | 2 |\n".to_string(),
                2 => "- [x] 已完成\n- [ ] 未完成\n".to_string(),
                3 => "$$\n\\frac{a}{b}\n$$\n".to_string(),
                _ => "![示例图](assets/示例.png)\n".to_string(),
            }),
        }
        if rng.below(3) == 0 {
            out.push('\n');
        }
    }
    out
}

pub fn mixed_samples(count: usize) -> Vec<String> {
    (0..count).map(mixed_sample).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_deterministic_and_non_empty() {
        let a = mixed_samples(100);
        let b = mixed_samples(100);
        assert_eq!(a, b);
        assert_eq!(a.len(), 100);
        assert!(a.iter().all(|s| !s.is_empty()));
        // the corpus must actually exercise CJK and markers
        assert!(a.iter().any(|s| s.contains("文")));
        assert!(a.iter().any(|s| s.contains("```")));
    }
}
