//! Vim editing for the New Message body: modes, motions, operators, counts, visual mode and
//! a few `:` commands. Pure: give it the text, the cursor (a char offset) and a key; it says
//! what to change. The app applies the change to its text box as one undo step, and runs
//! undo / redo / save / close itself.
//!
//! Keys are named like Vim's: "a", "G", "$", "<Esc>", "<CR>", "<BS>", "<C-r>".

/// Which mode the body is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Insert,
    Normal,
    Visual,
    VisualLine,
}

/// Replace chars `start..end` with `text` (offsets into the text before the change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Things the app does itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Undo,
    Redo,
    /// `:w`
    Save,
    /// `:q`
    Close,
    /// `:wq`, `:x`
    SaveClose,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Out {
    /// False: the key isn't Vim's here; let the text box (or window) have it.
    pub handled: bool,
    pub edit: Option<Edit>,
    /// Where the cursor goes, in the text after `edit`.
    pub cursor: Option<usize>,
    pub action: Option<Action>,
}

impl Out {
    fn eaten() -> Self {
        Out { handled: true, ..Default::default() }
    }
    fn to(cursor: usize) -> Self {
        Out { handled: true, cursor: Some(cursor), ..Default::default() }
    }
}

#[derive(Debug, Clone)]
pub struct Vim {
    pub mode: Mode,
    /// Keys of an unfinished command ("2d", "g").
    pending: String,
    /// The unnamed register and whether it holds whole lines.
    register: (String, bool),
    /// Where visual mode started.
    anchor: usize,
    /// The `:`, `/` or `?` line being typed (its prompt char and text).
    cmdline: Option<(char, String)>,
    /// The last f/F/t/T and its char, for `;` and `,`.
    last_find: Option<(char, char)>,
    /// The last search and whether it went forward, for `n` and `N`.
    last_search: Option<(String, bool)>,
    /// The last change for `.`: its keys, and the text typed after it if it entered Insert.
    last_change: Option<(String, Option<String>)>,
    /// A change that entered Insert mode: its keys and where typing started.
    recording: Option<(String, usize)>,
}

impl Default for Vim {
    fn default() -> Self {
        Vim {
            mode: Mode::Insert,
            pending: String::new(),
            register: (String::new(), false),
            anchor: 0,
            cmdline: None,
            last_find: None,
            last_search: None,
            last_change: None,
            recording: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// The text as chars, with line helpers.
struct Text {
    c: Vec<char>,
}

impl Text {
    fn len(&self) -> usize {
        self.c.len()
    }
    fn line_start(&self, pos: usize) -> usize {
        let pos = pos.min(self.len());
        self.c[..pos].iter().rposition(|&ch| ch == '\n').map_or(0, |i| i + 1)
    }
    /// Index of the line's '\n' (or the end of the text).
    fn line_end(&self, pos: usize) -> usize {
        let pos = pos.min(self.len());
        self.c[pos..].iter().position(|&ch| ch == '\n').map_or(self.len(), |i| pos + i)
    }
    /// The last char a Normal-mode cursor may sit on in this line.
    fn last_col(&self, pos: usize) -> usize {
        let (s, e) = (self.line_start(pos), self.line_end(pos));
        if e > s { e - 1 } else { s }
    }
    fn first_non_blank(&self, pos: usize) -> usize {
        let (s, e) = (self.line_start(pos), self.line_end(pos));
        (s..e).find(|&i| !matches!(self.c[i], ' ' | '\t')).unwrap_or(s)
    }
    fn line_no(&self, pos: usize) -> usize {
        self.c[..pos.min(self.len())].iter().filter(|&&ch| ch == '\n').count()
    }
    fn lines(&self) -> usize {
        self.c.iter().filter(|&&ch| ch == '\n').count() + 1
    }
    fn line_at(&self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        let mut seen = 0;
        for (i, &ch) in self.c.iter().enumerate() {
            if ch == '\n' {
                seen += 1;
                if seen == n {
                    return i + 1;
                }
            }
        }
        self.line_start(self.len())
    }
    /// Same column in the line `delta` lines away (clamped to that line).
    fn vertical(&self, pos: usize, delta: isize, col: usize) -> usize {
        let line = (self.line_no(pos) as isize + delta).clamp(0, self.lines() as isize - 1) as usize;
        let s = self.line_at(line);
        let e = self.line_end(s);
        (s + col).min(if e > s { e - 1 } else { s })
    }
    fn class_at(&self, i: usize) -> Class {
        self.c.get(i).map_or(Class::Space, |&ch| class(ch))
    }
    fn next_word_start(&self, mut i: usize) -> usize {
        let n = self.len();
        if i >= n {
            return n;
        }
        let start = self.class_at(i);
        if start != Class::Space {
            while i < n && self.class_at(i) == start {
                i += 1;
            }
        }
        while i < n && self.class_at(i) == Class::Space {
            i += 1;
        }
        i
    }
    fn word_end(&self, mut i: usize) -> usize {
        let n = self.len();
        if i + 1 >= n {
            return n.saturating_sub(1);
        }
        i += 1;
        while i < n && self.class_at(i) == Class::Space {
            i += 1;
        }
        let k = self.class_at(i);
        while i + 1 < n && self.class_at(i + 1) == k {
            i += 1;
        }
        i.min(n.saturating_sub(1))
    }
    fn prev_word_start(&self, mut i: usize) -> usize {
        if i == 0 {
            return 0;
        }
        i -= 1;
        while i > 0 && self.class_at(i) == Class::Space {
            i -= 1;
        }
        let k = self.class_at(i);
        while i > 0 && self.class_at(i - 1) == k {
            i -= 1;
        }
        i
    }
    fn slice(&self, a: usize, b: usize) -> String {
        self.c[a.min(self.len())..b.min(self.len())].iter().collect()
    }
}

/// Where a motion lands and how an operator treats the span.
struct Motion {
    target: usize,
    linewise: bool,
    /// The target char is part of the span (e, $, f…), not just its end.
    inclusive: bool,
}

impl Vim {
    pub fn new() -> Self {
        Self::default()
    }

    /// What to show under the body: the mode, a command being typed, or the `:` line.
    pub fn status(&self) -> String {
        if let Some((prompt, cmd)) = &self.cmdline {
            return format!("{prompt}{cmd}");
        }
        let mode = match self.mode {
            Mode::Insert => "-- INSERT --",
            Mode::Normal => "NORMAL",
            Mode::Visual => "-- VISUAL --",
            Mode::VisualLine => "-- VISUAL LINE --",
        };
        if self.pending.is_empty() { mode.to_string() } else { format!("{mode}   {}", self.pending) }
    }

    /// The selected span in visual mode, `start..end` (end exclusive).
    pub fn selection(&self, text: &str, cursor: usize) -> Option<(usize, usize)> {
        let t = Text { c: text.chars().collect() };
        self.visual_span(&t, cursor)
    }

    fn visual_span(&self, t: &Text, cursor: usize) -> Option<(usize, usize)> {
        let (a, b) = (self.anchor.min(cursor), self.anchor.max(cursor));
        match self.mode {
            Mode::Visual => Some((a, (b + 1).min(t.len()))),
            Mode::VisualLine => Some((t.line_start(a), t.line_end(b))),
            _ => None,
        }
    }

    pub fn key(&mut self, text: &str, cursor: usize, key: &str) -> Out {
        let t = Text { c: text.chars().collect() };
        let cursor = cursor.min(t.len());
        if self.cmdline.is_some() {
            return self.cmdline_key(&t, cursor, key);
        }
        match self.mode {
            Mode::Insert => {
                if key == "<Esc>" {
                    self.mode = Mode::Normal;
                    // What was typed since the change started, for `.` (typing forward; edits
                    // made by moving around aren't tracked).
                    if let Some((cmd, from)) = self.recording.take() {
                        let typed = if cursor >= from { t.slice(from, cursor) } else { String::new() };
                        self.last_change = Some((cmd, Some(typed)));
                    }
                    let back = if cursor > t.line_start(cursor) { cursor - 1 } else { cursor };
                    return Out::to(back);
                }
                Out::default()
            }
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_key(&t, cursor, key),
        }
    }

    fn cmdline_key(&mut self, t: &Text, cursor: usize, key: &str) -> Out {
        let (prompt, line) = self.cmdline.as_mut().expect("cmdline");
        let prompt = *prompt;
        match key {
            "<Esc>" => self.cmdline = None,
            "<BS>" => {
                if line.pop().is_none() {
                    self.cmdline = None;
                }
            }
            "<CR>" => {
                let (_, cmd) = self.cmdline.take().unwrap_or_default();
                if prompt != ':' {
                    // An empty search repeats the last one.
                    let pattern = if cmd.is_empty() { self.last_search.as_ref().map(|s| s.0.clone()).unwrap_or_default() } else { cmd };
                    if pattern.is_empty() {
                        return Out::eaten();
                    }
                    let forward = prompt == '/';
                    self.last_search = Some((pattern.clone(), forward));
                    return match search(t, cursor, &pattern, forward) {
                        Some(at) => Out::to(self.clamp(t, at)),
                        None => Out::eaten(),
                    };
                }
                let action = match cmd.trim() {
                    "w" => Some(Action::Save),
                    "q" | "q!" => Some(Action::Close),
                    "wq" | "x" => Some(Action::SaveClose),
                    _ => None,
                };
                return Out { handled: true, action, ..Default::default() };
            }
            k if k.chars().count() == 1 => line.push_str(k),
            _ => {}
        }
        Out::eaten()
    }

    /// Normal mode keeps the cursor on a char of the line, not past its end.
    fn clamp(&self, t: &Text, at: usize) -> usize {
        if self.mode == Mode::Normal { at.min(t.last_col(at)) } else { at.min(t.len().saturating_sub(1)) }
    }

    fn normal_key(&mut self, t: &Text, cursor: usize, key: &str) -> Out {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        match key {
            "<Esc>" => {
                if visual {
                    self.mode = Mode::Normal;
                    self.pending.clear();
                    return Out::to(cursor);
                }
                if !self.pending.is_empty() {
                    self.pending.clear();
                    return Out::eaten();
                }
                // Esc in Normal mode: the window's (asks to close).
                return Out::default();
            }
            "<C-r>" => {
                self.pending.clear();
                return Out { handled: true, action: Some(Action::Redo), ..Default::default() };
            }
            k if k.starts_with('<') && k != "<CR>" && k != "<BS>" => return Out::default(),
            _ => {}
        }
        if self.pending.is_empty() && (key == "/" || key == "?" || (key == ":" && !visual)) {
            self.cmdline = Some((key.chars().next().unwrap_or(':'), String::new()));
            return Out::eaten();
        }
        self.pending.push_str(match key {
            "<CR>" => "j",
            "<BS>" => "h",
            k => k,
        });
        let pending = self.pending.clone();
        match self.run(t, cursor, &pending) {
            Some(out) => {
                self.pending.clear();
                // Remember changes for `.` (not visual ones, nor `.` itself).
                if !visual && split_count(&pending).1 != "." {
                    if self.mode == Mode::Insert {
                        self.recording = Some((pending, out.cursor.unwrap_or(cursor)));
                    } else if out.edit.is_some() {
                        self.last_change = Some((pending, None));
                    }
                }
                out
            }
            None => Out::eaten(), // waiting for more keys
        }
    }

    /// Parse and run `cmd`; `None` while it's still incomplete. Unknown commands are dropped.
    fn run(&mut self, t: &Text, cursor: usize, cmd: &str) -> Option<Out> {
        let (count1, rest) = split_count(cmd);
        if rest.is_empty() {
            return None;
        }
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let n = count1.unwrap_or(1);

        // Visual mode: a text object (iw, a", ip…) becomes the selection.
        if visual && (rest.starts_with('i') || rest.starts_with('a')) {
            let mut chars = rest.chars();
            let kind = chars.next().unwrap_or('i');
            let obj = chars.next()?; // waiting for the object
            let Some((a, b, linewise)) = text_object(t, cursor, kind, obj) else { return Some(Out::eaten()) };
            if b <= a {
                return Some(Out::eaten());
            }
            self.anchor = a;
            if linewise {
                self.mode = Mode::VisualLine;
            }
            return Some(Out::to(b - 1));
        }
        if visual && let Some(out) = self.visual_command(t, cursor, rest) {
            return Some(out);
        }

        if rest == "." {
            return Some(self.repeat(t, cursor, count1));
        }

        // Operators: d c y, doubled for whole lines (dd), or with a motion (d2w, dw, d$).
        if let Some(op) = rest.chars().next().filter(|c| matches!(c, 'd' | 'c' | 'y')) {
            if visual {
                return Some(Out::eaten());
            }
            let after = &rest[1..];
            if after.is_empty() {
                return None;
            }
            if after.starts_with(op) {
                let last_line = (t.line_no(cursor) + n - 1).min(t.lines() - 1);
                let end = t.line_at(last_line);
                return Some(self.operate(t, cursor, op, cursor.min(end), Motion { target: end, linewise: true, inclusive: false }));
            }
            let (count2, motion) = split_count(after);
            if motion.is_empty() {
                return None;
            }
            let m = n * count2.unwrap_or(1);
            // cw changes to the end of the word, as in Vim.
            let motion = if op == 'c' && motion == "w" { "e" } else { motion };
            return match self.motion(t, cursor, motion, m, (count1.is_some() || count2.is_some()).then_some(m), true) {
                MotionResult::Pending => None,
                MotionResult::Invalid => Some(Out::eaten()),
                MotionResult::Done(mo) => Some(self.operate(t, cursor, op, cursor, mo)),
                MotionResult::Range(a, b, linewise) => {
                    Some(self.operate(t, cursor, op, a, Motion { target: b, linewise, inclusive: false }))
                }
            };
        }

        let simple = |this: &mut Vim, mode: Mode, at: usize| {
            this.mode = mode;
            Some(Out::to(at))
        };
        match rest {
            "i" => return simple(self, Mode::Insert, cursor),
            "a" => return simple(self, Mode::Insert, (cursor + 1).min(t.line_end(cursor))),
            "I" => return simple(self, Mode::Insert, t.first_non_blank(cursor)),
            "A" => return simple(self, Mode::Insert, t.line_end(cursor)),
            "o" | "O" => {
                self.mode = Mode::Insert;
                let at = if rest == "o" { t.line_end(cursor) } else { t.line_start(cursor) };
                let cursor_after = if rest == "o" { at + 1 } else { at };
                return Some(Out {
                    handled: true,
                    edit: Some(Edit { start: at, end: at, text: "\n".into() }),
                    cursor: Some(cursor_after),
                    action: None,
                });
            }
            "v" => {
                self.mode = Mode::Visual;
                self.anchor = cursor;
                return Some(Out::to(cursor));
            }
            "V" => {
                self.mode = Mode::VisualLine;
                self.anchor = cursor;
                return Some(Out::to(cursor));
            }
            "u" => return Some(Out { handled: true, action: Some(Action::Undo), ..Default::default() }),
            "x" => {
                let end = (cursor + n).min(t.line_end(cursor));
                if end == cursor {
                    return Some(Out::eaten());
                }
                return Some(self.operate(t, cursor, 'd', cursor, Motion { target: end, linewise: false, inclusive: false }));
            }
            "D" | "C" => {
                let op = if rest == "D" { 'd' } else { 'c' };
                let end = t.line_end(t.vertical(cursor, n as isize - 1, 0));
                return Some(self.operate(t, cursor, op, cursor, Motion { target: end, linewise: false, inclusive: false }));
            }
            "p" | "P" => return Some(self.put(t, cursor, rest == "p", n)),
            _ => {}
        }
        match self.motion(t, cursor, rest, n, count1, false) {
            MotionResult::Pending => None,
            MotionResult::Invalid | MotionResult::Range(..) => Some(Out::eaten()),
            MotionResult::Done(m) => Some(Out::to(self.clamp(t, m.target))),
        }
    }

    /// `.`: run the last change again (with a new count if one is given), typing the same
    /// text if it entered Insert mode, and end in Normal mode.
    fn repeat(&mut self, t: &Text, cursor: usize, count: Option<usize>) -> Out {
        let Some((cmd, typed)) = self.last_change.clone() else { return Out::eaten() };
        let cmd = match count {
            Some(c) => format!("{c}{}", split_count(&cmd).1),
            None => cmd,
        };
        let Some(mut out) = self.run(t, cursor, &cmd) else { return Out::eaten() };
        if self.mode != Mode::Insert {
            return out;
        }
        self.mode = Mode::Normal;
        let typed = typed.unwrap_or_default();
        let at = out.cursor.unwrap_or(cursor);
        let len = typed.chars().count();
        out.edit = Some(match out.edit.take() {
            Some(e) => {
                let inner: Vec<char> = e.text.chars().collect();
                let k = at.saturating_sub(e.start).min(inner.len());
                let text: String = inner[..k].iter().copied().chain(typed.chars()).chain(inner[k..].iter().copied()).collect();
                Edit { start: e.start, end: e.end, text }
            }
            None => Edit { start: at, end: at, text: typed },
        });
        out.cursor = Some(if len > 0 { at + len - 1 } else { at });
        out
    }

    fn visual_command(&mut self, t: &Text, cursor: usize, rest: &str) -> Option<Out> {
        let (a, b) = self.visual_span(t, cursor)?;
        let linewise = self.mode == Mode::VisualLine;
        let op = match rest {
            "d" | "x" => 'd',
            "y" => 'y',
            "c" | "s" => 'c',
            "v" | "V" => {
                let want = if rest == "v" { Mode::Visual } else { Mode::VisualLine };
                self.mode = if self.mode == want { Mode::Normal } else { want };
                return Some(Out::to(cursor));
            }
            _ => return None,
        };
        self.mode = Mode::Normal;
        Some(self.operate(t, cursor, op, a, Motion { target: b, linewise, inclusive: false }))
    }

    /// Apply operator `op` to the span between `from` and the motion's target.
    fn operate(&mut self, t: &Text, cursor: usize, op: char, from: usize, m: Motion) -> Out {
        let (mut a, mut b) = (from.min(m.target), from.max(m.target));
        if m.inclusive {
            b = (b + 1).min(t.len());
        }
        if m.linewise {
            a = t.line_start(a);
            b = t.line_end(b);
            self.register = (t.slice(a, b), true);
            match op {
                'y' => Out::to(cursor),
                'c' => {
                    self.mode = Mode::Insert;
                    Out { handled: true, edit: Some(Edit { start: a, end: b, text: String::new() }), cursor: Some(a), action: None }
                }
                _ => {
                    // Take the line break too: the one after, or before for the last line.
                    let (da, db) = if b < t.len() { (a, b + 1) } else if a > 0 { (a - 1, b) } else { (a, b) };
                    let after: Vec<char> = t.c[..da].iter().chain(t.c[db..].iter()).copied().collect();
                    let rest = Text { c: after };
                    let land = if b < t.len() { da } else { rest.line_start(da) };
                    Out {
                        handled: true,
                        edit: Some(Edit { start: da, end: db, text: String::new() }),
                        cursor: Some(rest.first_non_blank(land)),
                        action: None,
                    }
                }
            }
        } else {
            self.register = (t.slice(a, b), false);
            match op {
                'y' => Out::to(a),
                'c' => {
                    self.mode = Mode::Insert;
                    Out { handled: true, edit: Some(Edit { start: a, end: b, text: String::new() }), cursor: Some(a), action: None }
                }
                _ => {
                    let after: Vec<char> = t.c[..a].iter().chain(t.c[b..].iter()).copied().collect();
                    let rest = Text { c: after };
                    Out {
                        handled: true,
                        edit: Some(Edit { start: a, end: b, text: String::new() }),
                        cursor: Some(a.min(rest.last_col(a.min(rest.len())))),
                        action: None,
                    }
                }
            }
        }
    }

    fn put(&mut self, t: &Text, cursor: usize, after: bool, n: usize) -> Out {
        let (reg, linewise) = self.register.clone();
        if reg.is_empty() && !linewise {
            return Out::eaten();
        }
        if linewise {
            let block = vec![reg; n].join("\n");
            let (at, text, land) = if after {
                let at = t.line_end(cursor);
                (at, format!("\n{block}"), at + 1)
            } else {
                let at = t.line_start(cursor);
                (at, format!("{block}\n"), at)
            };
            let first: Vec<char> = text.chars().collect();
            let skip = if after { 1 } else { 0 };
            let indent = first[skip..].iter().take_while(|c| matches!(c, ' ' | '\t')).count();
            return Out { handled: true, edit: Some(Edit { start: at, end: at, text }), cursor: Some(land + indent), action: None };
        }
        let text = reg.repeat(n);
        let at = if after { (cursor + 1).min(t.line_end(cursor)) } else { cursor };
        let len = text.chars().count();
        Out { handled: true, edit: Some(Edit { start: at, end: at, text }), cursor: Some(at + len - 1), action: None }
    }

    fn motion(&mut self, t: &Text, cursor: usize, m: &str, n: usize, count: Option<usize>, for_op: bool) -> MotionResult {
        let col = cursor - t.line_start(cursor);
        let done = |target: usize, linewise: bool, inclusive: bool| MotionResult::Done(Motion { target, linewise, inclusive });
        let mut chars = m.chars();
        let (first, second, more) = (chars.next(), chars.next(), chars.next().is_some());
        // f F t T and a char; ; and , repeat the last one.
        if let (Some(kind @ ('f' | 'F' | 't' | 'T')), false) = (first, more) {
            let Some(ch) = second else { return MotionResult::Pending };
            self.last_find = Some((kind, ch));
            return find_char(t, cursor, kind, ch, n).map_or(MotionResult::Invalid, |(at, inc)| done(at, false, inc));
        }
        if m == ";" || m == "," {
            let Some((kind, ch)) = self.last_find else { return MotionResult::Invalid };
            let kind = if m == ";" { kind } else { reverse(kind) };
            return find_char(t, cursor, kind, ch, n).map_or(MotionResult::Invalid, |(at, inc)| done(at, false, inc));
        }
        if m == "n" || m == "N" {
            let Some((pattern, forward)) = self.last_search.clone() else { return MotionResult::Invalid };
            let forward = forward == (m == "n");
            let mut at = cursor;
            for _ in 0..n {
                match search(t, at, &pattern, forward) {
                    Some(p) => at = p,
                    None => return MotionResult::Invalid,
                }
            }
            return done(at, false, false);
        }
        // Text objects after an operator: iw, a", i(, ip…
        if for_op && let (Some(kind @ ('i' | 'a')), false) = (first, more) {
            let Some(obj) = second else { return MotionResult::Pending };
            return text_object(t, cursor, kind, obj).map_or(MotionResult::Invalid, |(a, b, lw)| MotionResult::Range(a, b, lw));
        }
        match m {
            "h" => done(cursor.saturating_sub(n).max(t.line_start(cursor)), false, false),
            "l" => {
                let limit = if for_op { t.line_end(cursor) } else { t.last_col(cursor) };
                done((cursor + n).min(limit), false, false)
            }
            "j" => done(t.vertical(cursor, n as isize, col), true, false),
            "k" => done(t.vertical(cursor, -(n as isize), col), true, false),
            "w" => {
                let mut i = cursor;
                for _ in 0..n {
                    i = t.next_word_start(i);
                }
                // dw on a line's last word stops at the line end.
                if for_op && t.line_no(i) != t.line_no(cursor) {
                    i = t.line_end(cursor);
                }
                done(i, false, false)
            }
            "e" => {
                let mut i = cursor;
                for _ in 0..n {
                    i = t.word_end(i);
                }
                done(i, false, true)
            }
            "b" => {
                let mut i = cursor;
                for _ in 0..n {
                    i = t.prev_word_start(i);
                }
                done(i, false, false)
            }
            "0" => done(t.line_start(cursor), false, false),
            "^" => done(t.first_non_blank(cursor), false, false),
            "$" => {
                let line = t.vertical(cursor, n as isize - 1, 0);
                let end = t.line_end(line);
                if for_op { done(end, false, false) } else { done(t.last_col(line), false, true) }
            }
            "g" => MotionResult::Pending,
            "gg" => done(t.first_non_blank(t.line_at(count.unwrap_or(1).saturating_sub(1).min(t.lines() - 1))), true, false),
            "G" => {
                let line = count.map_or(t.lines() - 1, |c| c.saturating_sub(1).min(t.lines() - 1));
                done(t.first_non_blank(t.line_at(line)), true, false)
            }
            _ => MotionResult::Invalid,
        }
    }
}

enum MotionResult {
    Pending,
    Invalid,
    Done(Motion),
    /// A text object: chars `a..b` (whole lines if the flag is set).
    Range(usize, usize, bool),
}

fn reverse(kind: char) -> char {
    match kind {
        'f' => 'F',
        'F' => 'f',
        't' => 'T',
        _ => 't',
    }
}

/// f / t: the `n`th `ch` after the cursor on this line (t stops one before); F / T before
/// it. Returns the landing spot and whether an operator includes it.
fn find_char(t: &Text, cursor: usize, kind: char, ch: char, n: usize) -> Option<(usize, bool)> {
    let (ls, le) = (t.line_start(cursor), t.line_end(cursor));
    match kind {
        'f' | 't' => {
            // `t` right before the char would stay put on repeat: look one further.
            let from = if kind == 't' { cursor + 2 } else { cursor + 1 };
            let p = (from.min(le)..le).filter(|&i| t.c[i] == ch).nth(n - 1)?;
            Some(if kind == 'f' { (p, true) } else { (p - 1, true) })
        }
        _ => {
            let upto = if kind == 'T' { cursor.saturating_sub(1) } else { cursor };
            let p = (ls..upto.max(ls)).rev().filter(|&i| t.c[i] == ch).nth(n - 1)?;
            Some(if kind == 'F' { (p, false) } else { (p + 1, false) })
        }
    }
}

/// Plain-text search from just past `from`, wrapping around. Case doesn't matter unless the
/// pattern has a capital (Vim's smartcase).
fn search(t: &Text, from: usize, pattern: &str, forward: bool) -> Option<usize> {
    let fold = !pattern.chars().any(char::is_uppercase);
    let norm = |c: char| if fold { c.to_lowercase().next().unwrap_or(c) } else { c };
    let pat: Vec<char> = pattern.chars().map(norm).collect();
    let hay: Vec<char> = t.c.iter().map(|&c| norm(c)).collect();
    if pat.is_empty() || pat.len() > hay.len() {
        return None;
    }
    let starts: Vec<usize> = (0..=hay.len() - pat.len()).filter(|&i| hay[i..i + pat.len()] == pat[..]).collect();
    if forward {
        starts.iter().copied().find(|&i| i > from).or_else(|| starts.first().copied())
    } else {
        starts.iter().rev().copied().find(|&i| i < from).or_else(|| starts.last().copied())
    }
}

/// The span of a text object at the cursor: `kind` 'i' (inner) or 'a' (around), and the
/// object: w W, quotes " ' `, brackets ( ) b { } B [ ] < >, p (paragraph, whole lines).
fn text_object(t: &Text, cursor: usize, kind: char, obj: char) -> Option<(usize, usize, bool)> {
    let around = kind == 'a';
    match obj {
        'w' | 'W' => {
            let (ls, le) = (t.line_start(cursor), t.line_end(cursor));
            if cursor >= le {
                return None;
            }
            let cls = |i: usize| if obj == 'W' && t.class_at(i) != Class::Space { Class::Word } else { t.class_at(i) };
            let k = cls(cursor);
            let (mut a, mut b) = (cursor, cursor + 1);
            while a > ls && cls(a - 1) == k {
                a -= 1;
            }
            while b < le && cls(b) == k {
                b += 1;
            }
            if around && k != Class::Space {
                if b < le && cls(b) == Class::Space {
                    while b < le && cls(b) == Class::Space {
                        b += 1;
                    }
                } else {
                    while a > ls && cls(a - 1) == Class::Space {
                        a -= 1;
                    }
                }
            }
            Some((a, b, false))
        }
        '"' | '\'' | '`' => {
            let (ls, le) = (t.line_start(cursor), t.line_end(cursor));
            let quotes: Vec<usize> = (ls..le).filter(|&i| t.c[i] == obj).collect();
            let pairs: Vec<(usize, usize)> = quotes.chunks(2).filter(|p| p.len() == 2).map(|p| (p[0], p[1])).collect();
            let &(q0, q1) = pairs.iter().find(|(a, b)| *a <= cursor && cursor <= *b).or_else(|| pairs.iter().find(|(a, _)| *a > cursor))?;
            Some(if around { (q0, q1 + 1, false) } else { (q0 + 1, q1, false) })
        }
        '(' | ')' | 'b' | '{' | '}' | 'B' | '[' | ']' | '<' | '>' => {
            let (open, close) = match obj {
                '(' | ')' | 'b' => ('(', ')'),
                '{' | '}' | 'B' => ('{', '}'),
                '[' | ']' => ('[', ']'),
                _ => ('<', '>'),
            };
            // Back to the unmatched opener (the cursor may sit on either bracket)…
            let mut depth = 0;
            let mut i = cursor.min(t.len().saturating_sub(1)) as isize;
            if t.c.get(i as usize) == Some(&close) {
                i -= 1;
            }
            let start = loop {
                if i < 0 {
                    return None;
                }
                let ch = t.c[i as usize];
                if ch == close {
                    depth += 1;
                } else if ch == open {
                    if depth == 0 {
                        break i as usize;
                    }
                    depth -= 1;
                }
                i -= 1;
            };
            // …and forward to its closer.
            let mut depth = 0;
            let end = (start + 1..t.len()).find(|&j| {
                let ch = t.c[j];
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                false
            })?;
            Some(if around { (start, end + 1, false) } else { (start + 1, end, false) })
        }
        'p' => {
            let blank = |line: usize| {
                let s = t.line_at(line);
                (s..t.line_end(s)).all(|i| t.c[i].is_whitespace())
            };
            let here = t.line_no(cursor);
            let kind_here = blank(here);
            let (mut first, mut last) = (here, here);
            while first > 0 && blank(first - 1) == kind_here {
                first -= 1;
            }
            while last + 1 < t.lines() && blank(last + 1) == kind_here {
                last += 1;
            }
            if around {
                while last + 1 < t.lines() && blank(last + 1) != kind_here {
                    last += 1;
                }
            }
            let a = t.line_at(first);
            Some((a, t.line_end(t.line_at(last)), true))
        }
        _ => None,
    }
}

/// A leading count ("12dw" → 12, "dw"); a lone "0" is the motion, not a count.
fn split_count(s: &str) -> (Option<usize>, &str) {
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || s.starts_with('0') {
        return (None, s);
    }
    (s[..digits].parse().ok(), &s[digits..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Type `keys` (space-separated Vim key names) from Normal mode at `cursor`; returns the
    /// text and cursor afterwards.
    fn run(text: &str, cursor: usize, keys: &str) -> (String, usize, Vim) {
        let mut v = Vim { mode: Mode::Normal, ..Vim::default() };
        let mut text = text.to_string();
        let mut cur = cursor;
        for k in keys.split(' ') {
            let out = v.key(&text, cur, k);
            if !out.handled && v.mode == Mode::Insert {
                // Insert mode: the text box types it.
                let mut c: Vec<char> = text.chars().collect();
                for ch in k.chars() {
                    c.insert(cur, ch);
                    cur += 1;
                }
                text = c.into_iter().collect();
                continue;
            }
            if let Some(e) = out.edit {
                let c: Vec<char> = text.chars().collect();
                text = c[..e.start].iter().chain(e.text.chars().collect::<Vec<_>>().iter()).chain(c[e.end..].iter()).collect();
            }
            if let Some(c) = out.cursor {
                cur = c;
            }
        }
        (text, cur, v)
    }

    #[test]
    fn insert_then_esc_steps_back() {
        let mut v = Vim::new();
        assert_eq!(v.mode, Mode::Insert, "a new message starts in Insert");
        assert!(!v.key("hello", 5, "x").handled, "typing goes to the text box");
        assert_eq!(v.key("hello", 5, "<Esc>"), Out::to(4));
        assert_eq!(v.mode, Mode::Normal);
        assert!(!v.key("hello", 4, "<Esc>").handled, "Esc in Normal is the window's (close)");
    }

    #[test]
    fn motions() {
        let t = "one two three\n  four five\nsix";
        assert_eq!(run(t, 0, "w").1, 4);
        assert_eq!(run(t, 0, "2 w").1, 8);
        assert_eq!(run(t, 0, "e").1, 2);
        assert_eq!(run(t, 8, "b").1, 4);
        assert_eq!(run(t, 0, "$").1, 12);
        assert_eq!(run(t, 5, "0").1, 0);
        assert_eq!(run(t, 0, "j").1, 14);
        assert_eq!(run(t, 12, "j").1, 24, "clamped to the shorter line");
        assert_eq!(run(t, 16, "^").1, 16);
        assert_eq!(run(t, 0, "G").1, 26);
        assert_eq!(run(t, 26, "g g").1, 0);
        assert_eq!(run(t, 0, "2 G").1, 16, "2G: line 2, first non-blank");
        assert_eq!(run(t, 0, "l l l").1, 3);
        assert_eq!(run(t, 12, "l").1, 12, "l stops at the line's end");
    }

    #[test]
    fn delete_change_yank_put() {
        let t = "one two three";
        assert_eq!(run(t, 0, "d w").0, "two three");
        assert_eq!(run(t, 0, "d 2 w").0, "three");
        assert_eq!(run(t, 0, "2 d w").0, "three");
        assert_eq!(run(t, 4, "d e").0, "one  three");
        assert_eq!(run(t, 4, "D").0, "one ");
        assert_eq!(run(t, 0, "x x").0, "e two three");
        assert_eq!(run(t, 0, "3 x").0, " two three");
        let (s, c, v) = run(t, 4, "c w T W O <Esc>");
        assert_eq!((s.as_str(), v.mode), ("one TWO three", Mode::Normal));
        assert_eq!(c, 6);
        assert_eq!(run(t, 8, "d w").0, "one two ", "dw on the last word stops at the end");
        assert_eq!(run(t, 0, "y w $ p").0, "one two threeone ");
        assert_eq!(run(t, 4, "d w P").0, "one two three");
    }

    #[test]
    fn whole_lines() {
        let t = "a\nb\nc";
        assert_eq!(run(t, 2, "d d").0, "a\nc");
        let (s, c, _) = run(t, 4, "d d");
        assert_eq!((s.as_str(), c), ("a\nb", 2), "last line takes the break before it");
        assert_eq!(run(t, 0, "2 d d").0, "c");
        assert_eq!(run(t, 0, "d j").0, "c");
        assert_eq!(run(t, 0, "y y j p").0, "a\nb\na\nc");
        assert_eq!(run(t, 2, "y y P").0, "a\nb\nb\nc");
        assert_eq!(run(t, 2, "d d p").0, "a\nc\nb");
        let (s, _, v) = run(t, 2, "c c x <Esc>");
        assert_eq!((s.as_str(), v.mode), ("a\nx\nc", Mode::Normal));
    }

    #[test]
    fn opening_lines() {
        assert_eq!(run("a\nb", 0, "o x").0, "a\nx\nb");
        assert_eq!(run("a\nb", 2, "O x").0, "a\nx\nb");
        assert_eq!(run("ab", 0, "A x").0, "abx");
        assert_eq!(run("  ab", 3, "I x").0, "  xab");
        assert_eq!(run("ab", 0, "a x").0, "axb");
    }

    #[test]
    fn visual() {
        let t = "one two three";
        assert_eq!(run(t, 4, "v e d").0, "one  three");
        let (s, _, v) = run(t, 4, "v e c 2 <Esc>");
        assert_eq!((s.as_str(), v.mode), ("one 2 three", Mode::Normal));
        assert_eq!(run(t, 4, "v e y $ p").0, "one two threetwo");
        assert_eq!(run("a\nb\nc", 2, "V j d").0, "a");
        assert_eq!(run(t, 4, "v <Esc> x").0, "one wo three");
        let mut v = Vim { mode: Mode::Normal, ..Vim::default() };
        v.key(t, 4, "v");
        assert_eq!(v.selection(t, 6), Some((4, 7)));
    }

    #[test]
    fn find_on_the_line() {
        let t = "one two three"; // o0 n1 e2 _3 t4 w5 o6 _7 t8 h9 r10 e11 e12
        assert_eq!(run(t, 0, "f t").1, 4);
        assert_eq!(run(t, 0, "f t ;").1, 8);
        assert_eq!(run(t, 0, "f t ; ,").1, 4);
        assert_eq!(run(t, 0, "2 f t").1, 8);
        assert_eq!(run(t, 0, "t t").1, 3);
        assert_eq!(run(t, 0, "t t ;").1, 7, "; after t looks past the char it stopped at");
        assert_eq!(run(t, 12, "F t").1, 8);
        assert_eq!(run(t, 12, "T t").1, 9);
        assert_eq!(run(t, 0, "f z").1, 0, "not on the line: stay");
        assert_eq!(run(t, 0, "d f o").0, " three");
        assert_eq!(run(t, 0, "d t t").0, "two three");
        assert_eq!(run(t, 12, "d F t").0, "one two e");
        assert_eq!(run(t, 0, "c t w X <Esc>").0, "Xwo three");
    }

    #[test]
    fn text_objects() {
        let t = "one two three";
        assert_eq!(run(t, 5, "d i w").0, "one  three");
        assert_eq!(run(t, 5, "d a w").0, "one three");
        assert_eq!(run(t, 10, "d a w").0, "one two", "last word takes the space before it");
        assert_eq!(run(t, 5, "c i w X <Esc>").0, "one X three");
        assert_eq!(run("a foo.bar b", 3, "d i W").0, "a  b");
        let q = "x \"ab cd\" y";
        assert_eq!(run(q, 4, "d i \"").0, "x \"\" y");
        assert_eq!(run(q, 4, "d a \"").0, "x  y");
        assert_eq!(run(q, 0, "d i \"").0, "x \"\" y", "before the quotes: the next pair");
        let b = "f(a, (b), c) z"; // f0 (1 a2 ,3 _4 (5 b6 )7 ,8 _9 c10 )11
        assert_eq!(run(b, 3, "d i (").0, "f() z");
        assert_eq!(run(b, 6, "d i b").0, "f(a, (), c) z");
        assert_eq!(run(b, 6, "d a (").0, "f(a, , c) z");
        assert_eq!(run(b, 11, "d i )").0, "f() z", "on the closing bracket");
        assert_eq!(run("{\n  x\n}", 4, "d i {").0, "{}");
        assert_eq!(run("a\nb\n\nc", 0, "d a p").0, "c");
        assert_eq!(run("a\nb\n\nc", 2, "d i p").0, "\nc");
        assert_eq!(run("a\nb\n\nc", 0, "y i p G p").0, "a\nb\n\nc\na\nb");
        assert_eq!(run(t, 5, "v i w d").0, "one  three");
        assert_eq!(run(b, 6, "v a ( d").0, "f(a, , c) z");
    }

    #[test]
    fn dot_repeats_the_last_change() {
        assert_eq!(run("a b c d", 0, "d w .").0, "c d");
        assert_eq!(run("abcdef", 0, "x .").0, "cdef");
        assert_eq!(run("abcdef", 0, "x 3 .").0, "ef", "a count replaces the old one");
        assert_eq!(run("one two", 0, "c w X <Esc> w .").0, "X X");
        assert_eq!(run("a\nb", 0, "A ! <Esc> j .").0, "a!\nb!");
        assert_eq!(run("a\nb", 0, "o x <Esc> j .").0, "a\nx\nb\nx");
        let (_, _, v) = run("a b c", 0, "d w u");
        assert_eq!(v.last_change, Some(("dw".to_string(), None)), "undo isn't a change; . still means dw");
        let (s, c, v) = run("one two", 0, "c w X <Esc> w .");
        assert_eq!((s.as_str(), c, v.mode), ("X X", 2, Mode::Normal));
    }

    #[test]
    fn search_in_the_body() {
        let t = "one two one two"; // o0 … t4 … o8 … t12
        assert_eq!(run(t, 0, "/ t w o <CR>").1, 4);
        assert_eq!(run(t, 0, "/ t w o <CR> n").1, 12);
        assert_eq!(run(t, 0, "/ t w o <CR> n n").1, 4, "wraps around");
        assert_eq!(run(t, 0, "/ t w o <CR> N").1, 12, "N goes back, wrapping");
        assert_eq!(run(t, 12, "? o n e <CR>").1, 8);
        assert_eq!(run(t, 0, "/ T W O <CR>").1, 0, "a capital makes it case-sensitive: no match");
        assert_eq!(run("One one", 0, "/ o n e <CR>").1, 4);
        assert_eq!(run(t, 0, "/ t w o <CR> d n").0, "one two", "dn deletes up to the next match");
        let mut v = Vim { mode: Mode::Normal, ..Vim::default() };
        v.key(t, 0, "/");
        v.key(t, 0, "x");
        assert_eq!(v.status(), "/x");
    }

    #[test]
    fn undo_redo_and_commands() {
        let mut v = Vim { mode: Mode::Normal, ..Vim::default() };
        assert_eq!(v.key("a", 0, "u").action, Some(Action::Undo));
        assert_eq!(v.key("a", 0, "<C-r>").action, Some(Action::Redo));
        for (cmd, want) in [("w", Action::Save), ("q", Action::Close), ("wq", Action::SaveClose), ("x", Action::SaveClose)] {
            v.key("a", 0, ":");
            for ch in cmd.chars() {
                v.key("a", 0, &ch.to_string());
            }
            assert_eq!(v.status(), format!(":{cmd}"));
            assert_eq!(v.key("a", 0, "<CR>").action, Some(want), ":{cmd}");
        }
        v.key("a", 0, ":");
        assert!(v.key("a", 0, "<Esc>").handled);
        assert_eq!(v.status(), "NORMAL");
        v.key("a", 0, "2");
        assert_eq!(v.status(), "NORMAL   2");
    }
}
