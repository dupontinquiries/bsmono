pub mod kernel;
pub mod lu;
pub mod newton;
pub mod sparse;
pub mod sum;

pub use lu::{Lu, LuError};
pub use kernel::{axpy, dot, dot_compensated, norm2, norm_inf};
pub use newton::{solve, NewtonOpts, Outcome, Report, System};
pub use sparse::{cg, Csr, CgReport};
pub use sum::{sum_exact, sum_neumaier, sum_pairwise};
