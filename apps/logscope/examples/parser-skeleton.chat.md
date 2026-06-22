## You
write a recursive-descent parser for simple arithmetic (+ - * / and parens)

## Assistant
Here's the structure:
```rust
enum Expr { Num(f64), Add(Box<Expr>, Box<Expr>), Mul(Box<Expr>, Box<Expr>) }

fn parse(input: &str) -> Result<Expr, String> { todo!() }
fn eval(e: &Expr) -> f64 { todo!() }
```
