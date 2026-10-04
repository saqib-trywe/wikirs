//! Math as Unicode (markdown.md#presentation): `x^2` → x², `\alpha` → α,
//! `\frac{a}{b}` → a⁄b. An approximation both UIs share; a command it doesn't
//! know is kept as written.

use std::fmt::Write as _;

/// `src` (the inside of `$…$` or `$$…$$`) as Unicode.
#[must_use]
pub fn to_unicode(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    convert(&chars, &mut i, &mut out, false);
    // Spaces don't matter in TeX math: keep single ones for readability.
    let mut collapsed = String::with_capacity(out.len());
    for c in out.trim().chars() {
        if !(c == ' ' && collapsed.ends_with(' ')) {
            collapsed.push(c);
        }
    }
    collapsed
}

/// Converts until the end, or (with `group`) the closing `}`.
fn convert(chars: &[char], i: &mut usize, out: &mut String, group: bool) {
    while let Some(&c) = chars.get(*i) {
        *i += 1;
        match c {
            '}' if group => return,
            '{' => convert(chars, i, out, true),
            '\\' => command(chars, i, out),
            '^' => script(chars, i, out, superscript, '^'),
            '_' => script(chars, i, out, subscript, '_'),
            '~' => out.push(' '),
            c => out.push(c),
        }
    }
}

/// The next argument: a `{…}` group or one character (or command), converted.
fn argument(chars: &[char], i: &mut usize) -> String {
    while chars.get(*i).is_some_and(|c| *c == ' ') {
        *i += 1;
    }
    let mut out = String::new();
    match chars.get(*i) {
        Some('{') => {
            *i += 1;
            convert(chars, i, &mut out, true);
        }
        Some('\\') => {
            *i += 1;
            command(chars, i, &mut out);
        }
        Some(&c) => {
            *i += 1;
            out.push(c);
        }
        None => {}
    }
    out
}

fn script(
    chars: &[char],
    i: &mut usize,
    out: &mut String,
    map: fn(char) -> Option<char>,
    mark: char,
) {
    let arg = argument(chars, i);
    if arg.is_empty() {
        out.push(mark);
        return;
    }
    match arg.chars().map(map).collect::<Option<String>>() {
        Some(small) => out.push_str(&small),
        _ if arg.chars().count() == 1 => {
            out.push(mark);
            out.push_str(&arg);
        }
        _ => {
            out.push(mark);
            out.push('(');
            out.push_str(&arg);
            out.push(')');
        }
    }
}

fn command(chars: &[char], i: &mut usize, out: &mut String) {
    let start = *i;
    while chars.get(*i).is_some_and(char::is_ascii_alphabetic) {
        *i += 1;
    }
    let name: String = chars[start..*i].iter().collect();
    if name.is_empty() {
        // `\,` `\;` `\ ` space; `\{` `\}` `\\` the character itself.
        match chars.get(*i) {
            Some(',' | ';' | ':' | '!' | ' ') => out.push(' '),
            Some('\\') => out.push('\n'),
            Some(&c) => out.push(c),
            None => out.push('\\'),
        }
        *i += 1;
        return;
    }
    match name.as_str() {
        "frac" | "dfrac" | "tfrac" => {
            let (num, den) = (argument(chars, i), argument(chars, i));
            let simple = |s: &str| s.chars().all(char::is_alphanumeric);
            if simple(&num) && simple(&den) {
                let _ = write!(out, "{num}⁄{den}");
            } else {
                let _ = write!(out, "({num})/({den})");
            }
        }
        "sqrt" => {
            let arg = argument(chars, i);
            if arg.chars().count() == 1 {
                let _ = write!(out, "√{arg}");
            } else {
                let _ = write!(out, "√({arg})");
            }
        }
        "mathbb" => {
            let arg = argument(chars, i);
            out.extend(arg.chars().map(|c| match c {
                'R' => 'ℝ',
                'N' => 'ℕ',
                'Z' => 'ℤ',
                'Q' => 'ℚ',
                'C' => 'ℂ',
                c => c,
            }));
        }
        "text" | "mathrm" | "mathit" | "mathbf" | "operatorname" | "textrm" | "mathsf" => {
            out.push_str(&argument(chars, i));
        }
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" => {}
        "quad" | "qquad" => out.push_str("  "),
        name => {
            if let Some(s) = symbol(name) {
                out.push_str(s);
            } else {
                out.push('\\');
                out.push_str(name);
            }
        }
    }
}

/// LaTeX commands with a Unicode (or plain-text) form.
const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "θ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("oint", "∮"),
    ("infty", "∞"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("pm", "±"),
    ("mp", "∓"),
    ("times", "×"),
    ("cdot", "·"),
    ("div", "÷"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("equiv", "≡"),
    ("sim", "∼"),
    ("propto", "∝"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("gets", "←"),
    ("Rightarrow", "⇒"),
    ("implies", "⇒"),
    ("Leftarrow", "⇐"),
    ("leftrightarrow", "↔"),
    ("Leftrightarrow", "⇔"),
    ("iff", "⇔"),
    ("mapsto", "↦"),
    ("in", "∈"),
    ("notin", "∉"),
    ("ni", "∋"),
    ("subset", "⊂"),
    ("subseteq", "⊆"),
    ("supset", "⊃"),
    ("supseteq", "⊇"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("setminus", "∖"),
    ("emptyset", "∅"),
    ("varnothing", "∅"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("neg", "¬"),
    ("lnot", "¬"),
    ("land", "∧"),
    ("wedge", "∧"),
    ("lor", "∨"),
    ("vee", "∨"),
    ("oplus", "⊕"),
    ("otimes", "⊗"),
    ("ldots", "…"),
    ("dots", "…"),
    ("cdots", "⋯"),
    ("vdots", "⋮"),
    ("ddots", "⋱"),
    ("langle", "⟨"),
    ("rangle", "⟩"),
    ("lfloor", "⌊"),
    ("rfloor", "⌋"),
    ("lceil", "⌈"),
    ("rceil", "⌉"),
    ("degree", "°"),
    ("circ", "∘"),
    ("star", "⋆"),
    ("ast", "∗"),
    ("hbar", "ℏ"),
    ("ell", "ℓ"),
    ("Re", "ℜ"),
    ("Im", "ℑ"),
    ("aleph", "ℵ"),
    ("perp", "⊥"),
    ("parallel", "∥"),
    ("angle", "∠"),
    ("triangle", "△"),
    ("sin", "sin"),
    ("cos", "cos"),
    ("tan", "tan"),
    ("log", "log"),
    ("ln", "ln"),
    ("exp", "exp"),
    ("lim", "lim"),
    ("max", "max"),
    ("min", "min"),
    ("det", "det"),
];

fn symbol(name: &str) -> Option<&'static str> {
    SYMBOLS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

fn superscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' | '−' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        'x' => 'ˣ',
        'T' => 'ᵀ',
        '*' => '*',
        '′' => '′',
        _ => return None,
    })
}

fn subscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' | '−' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'o' => 'ₒ',
        'x' => 'ₓ',
        'h' => 'ₕ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'p' => 'ₚ',
        's' => 'ₛ',
        't' => 'ₜ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'r' => 'ᵣ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::to_unicode;

    #[test]
    fn common_latex_becomes_unicode() {
        let cases = [
            ("x^2 + y^{10}", "x² + y¹⁰"),
            ("a_i + b_{n+1}", "aᵢ + bₙ₊₁"),
            (r"\alpha \to \beta", "α → β"),
            (r"\frac{a}{b}", "a⁄b"),
            (r"\frac{a+1}{2}", "(a+1)/(2)"),
            (r"\sqrt{2} \sqrt{x+y}", "√2 √(x+y)"),
            (r"\sum_{i=1}^{n} i", "∑ᵢ₌₁ⁿ i"),
            (r"x \in \mathbb{R}", "x ∈ ℝ"),
            (r"\text{if } x \leq 0", "if x ≤ 0"),
            (r"\left( a \right)", "( a )"),
            (r"e^{i\pi}", "e^(iπ)"),
            (r"x_{max}", "xₘₐₓ"),
            (r"x_{bc}", "x_(bc)"),
            (r"\unknown{x}", r"\unknownx"),
            (r"a \, b", "a b"),
            (r"\{1, 2\}", "{1, 2}"),
            ("", ""),
            (r"x^", "x^"),
        ];
        for (latex, want) in cases {
            assert_eq!(to_unicode(latex), want, "{latex}");
        }
    }
}
