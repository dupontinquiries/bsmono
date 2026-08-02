pub mod kernel;
pub mod newton;
pub mod sum;

pub use kernel::{axpy, dot, dot_compensated, norm2, norm_inf};
pub use newton::{solve, NewtonOpts, Outcome, Report, System};
pub use sum::{sum_exact, sum_neumaier, sum_pairwise};
