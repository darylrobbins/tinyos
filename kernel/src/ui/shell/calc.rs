//! Tiny recursive-descent calculator for the command bar: + - * / ( ).

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i] == b' ' {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> u8 {
        self.skip_ws();
        if self.i < self.b.len() {
            self.b[self.i]
        } else {
            0
        }
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                b'+' => {
                    self.i += 1;
                    v += self.term()?;
                }
                b'-' => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => return Some(v),
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        loop {
            match self.peek() {
                b'*' => {
                    self.i += 1;
                    v *= self.factor()?;
                }
                b'/' => {
                    self.i += 1;
                    let d = self.factor()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                _ => return Some(v),
            }
        }
    }

    fn factor(&mut self) -> Option<f64> {
        match self.peek() {
            b'(' => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != b')' {
                    return None;
                }
                self.i += 1;
                Some(v)
            }
            b'-' => {
                self.i += 1;
                Some(-self.factor()?)
            }
            _ => self.num(),
        }
    }

    fn num(&mut self) -> Option<f64> {
        self.skip_ws();
        let start = self.i;
        let mut seen_dot = false;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'0'..=b'9' => self.i += 1,
                b'.' if !seen_dot => {
                    seen_dot = true;
                    self.i += 1;
                }
                _ => break,
            }
        }
        if self.i == start {
            return None;
        }
        core::str::from_utf8(&self.b[start..self.i])
            .ok()?
            .parse()
            .ok()
    }
}

/// Safe by construction: this grammar admits only numeric literals and
/// arithmetic operators — nothing is executed or dereferenced.
pub fn eval(expr: &str) -> Option<f64> {
    let mut p = P {
        b: expr.as_bytes(),
        i: 0,
    };
    let v = p.expr()?;
    p.skip_ws();
    (p.i == p.b.len() && v.is_finite()).then_some(v)
}
