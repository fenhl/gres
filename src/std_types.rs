use crate::{
    Percent,
    Progress,
};

impl Progress for bool {
    fn progress(&self) -> Percent {
        if *self { Percent::MAX } else { Percent::default() }
    }
}

impl Progress for (usize, usize) {
    fn progress(&self) -> Percent {
        Percent::fraction(self.0, self.1)
    }
}
