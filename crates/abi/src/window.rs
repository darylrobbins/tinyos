//! Window protocol v0 (app <-> shell over the SHELL channel).
//!
//! Message = u32 LE opcode + payload. The surface travels as a MemObj handle
//! on OPEN (BGRA, stride = width). One window per connection.

// app -> shell
pub const OP_OPEN: u32 = 1; // {w:u32, h:u32, title:u32-len+utf8} + MemObj
pub const OP_PRESENT: u32 = 3; // {x,y,w,h: u32} damage

// shell -> app
pub const OP_OPENED: u32 = 2; // {status:u32}
pub const OP_CHAR: u32 = 16; // {c:u32}
pub const OP_KEY: u32 = 17; // {code:u16, down:u8}
pub const OP_CLOSE_REQ: u32 = 18;
pub const OP_POINTER: u32 = 19; // {x:i32, y:i32} body-local
pub const OP_BUTTON: u32 = 20; // {down:u8, x:i32, y:i32}
pub const OP_CTRL: u32 = 21; // {code:u32} key pressed with Ctrl held
