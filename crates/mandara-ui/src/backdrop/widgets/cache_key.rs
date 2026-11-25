//! Hash borrowed widget data without formatting a temporary string per frame.

use std::fmt::{Debug,Write as _};
use std::hash::{DefaultHasher,Hasher as _};

struct Writer(DefaultHasher);
impl std::fmt::Write for Writer {
    fn write_str(&mut self,value: &str) -> std::fmt::Result { self.0.write(value.as_bytes());Ok(()) }
}
pub(super) fn debug(value: &impl Debug) -> u64 {
    let mut writer = Writer(DefaultHasher::new());
    write!(&mut writer,"{value:?}").expect("the hash writer cannot fail");
    writer.0.finish()
}
