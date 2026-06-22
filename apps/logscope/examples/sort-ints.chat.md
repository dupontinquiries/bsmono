# logscope chat

## You

write a rust crate that sorts a vec of 1000 random ints

## Assistant

I'll scaffold a small crate. The core is one function over a `Vec<i32>` — sort in place, return it — plus a test that checks the result is non-decreasing. Want the simplest correct version first, then we can branch into faster or fancier approaches.

## You

start simple, then let's fan out and see which approach holds up
