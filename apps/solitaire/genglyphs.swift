// Generate 8bpp alpha-mask glyphs: card ranks/suits for the solitaire app and
// a proportional UI font for the SDK, all rasterized with CoreText.
// Usage: swift apps/solitaire/genglyphs.swift assets/geist-semibold.ttf \
//            apps/solitaire/src/glyphs.rs apps/sdk/src/uifont.rs
import Foundation
import CoreText
import CoreGraphics

let geistPath = CommandLine.arguments[1]
let appOut = CommandLine.arguments[2]
let sdkOut = CommandLine.arguments[3]

CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: geistPath) as CFURL, .process, nil)

let pad = 2

func line(_ text: String, _ font: CTFont) -> CTLine {
    let attr = [
        kCTFontAttributeName: font,
        kCTForegroundColorFromContextAttributeName: kCFBooleanTrue!,
    ] as CFDictionary
    return CTLineCreateWithAttributedString(CFAttributedStringCreate(nil, text as CFString, attr)!)
}

func rasterize(_ l: CTLine, _ bounds: CGRect) -> (Int, Int, [UInt8]) {
    let w = Int(ceil(bounds.width)) + 2 * pad
    let h = Int(ceil(bounds.height)) + 2 * pad
    var buf = [UInt8](repeating: 0, count: w * h)
    buf.withUnsafeMutableBytes { p in
        let ctx = CGContext(data: p.baseAddress, width: w, height: h,
                            bitsPerComponent: 8, bytesPerRow: w,
                            space: CGColorSpaceCreateDeviceGray(),
                            bitmapInfo: CGImageAlphaInfo.none.rawValue)!
        ctx.setAllowsAntialiasing(true)
        ctx.setShouldSmoothFonts(false)
        ctx.setFillColor(gray: 1.0, alpha: 1.0)
        ctx.textPosition = CGPoint(x: -bounds.minX + CGFloat(pad), y: -bounds.minY + CGFloat(pad))
        CTLineDraw(l, ctx)
    }
    return (w, h, buf)
}

func render(_ text: String, font: CTFont) -> (Int, Int, [UInt8]) {
    let l = line(text, font)
    return rasterize(l, CTLineGetImageBounds(l, nil))
}

func bytes(_ data: [UInt8]) -> String {
    data.map { String($0) }.joined(separator: ",")
}

// ---- Solitaire card glyphs -------------------------------------------------

let geist = CTFontCreateWithName("Geist SemiBold" as CFString, 24, nil)
let geistSmall = CTFontCreateWithName("Geist SemiBold" as CFString, 15, nil)
let geistBanner = CTFontCreateWithName("Geist SemiBold" as CFString, 54, nil)
let symbols = CTFontCreateWithName("Apple Symbols" as CFString, 20, nil)
let symbolsBig = CTFontCreateWithName("Apple Symbols" as CFString, 52, nil)
let symbolsMid = CTFontCreateWithName("Apple Symbols" as CFString, 30, nil)

var app = """
//! Generated glyph alpha masks — do not edit by hand.
//! Ranks: Geist SemiBold 24px (small: 15px). Suits + recycle: Apple Symbols.
//! Regenerate: swift apps/solitaire/genglyphs.swift assets/geist-semibold.ttf \\
//!     apps/solitaire/src/glyphs.rs apps/sdk/src/uifont.rs
#![allow(dead_code)]

pub struct Glyph {
    pub w: i32,
    pub h: i32,
    pub data: &'static [u8],
}

"""

func emitApp(_ name: String, _ g: (Int, Int, [UInt8])) {
    app += "pub static \(name): Glyph = Glyph { w: \(g.0), h: \(g.1), data: &[\(bytes(g.2))] };\n"
}

let ranks = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"]
for (i, r) in ranks.enumerated() {
    emitApp("RANK_\(i)", render(r, font: geist))
    emitApp("RANK_SM_\(i)", render(r, font: geistSmall))
}
let suits = ["\u{2660}": "SPADE", "\u{2665}": "HEART", "\u{2666}": "DIAMOND", "\u{2663}": "CLUB"]
for (ch, name) in suits {
    emitApp("\(name)_SM", render(ch, font: symbols))
    emitApp("\(name)_BIG", render(ch, font: symbolsBig))
}
emitApp("RECYCLE", render("\u{27F3}", font: symbolsMid))
emitApp("BANNER_WON", render("You won!", font: geistBanner))
let digitFont = CTFontCreateWithName("Geist SemiBold" as CFString, 13, nil)
for d in 0...9 { emitApp("DIGIT_\(d)", render(String(d), font: digitFont)) }

app += "\npub static DIGITS: [&Glyph; 10] = [" + (0...9).map { "&DIGIT_\($0)" }.joined(separator: ", ") + "];\n"
app += "pub static RANKS: [&Glyph; 13] = [" + (0..<13).map { "&RANK_\($0)" }.joined(separator: ", ") + "];\n"
app += "pub static RANKS_SM: [&Glyph; 13] = [" + (0..<13).map { "&RANK_SM_\($0)" }.joined(separator: ", ") + "];\n"

try! app.write(toFile: appOut, atomically: true, encoding: .utf8)
print("wrote \(appOut)")

// ---- SDK UI font (proportional, baseline-aligned) --------------------------

let uiFont = CTFontCreateWithName("Geist SemiBold" as CFString, 15, nil)
let ascent = Int(round(CTFontGetAscent(uiFont)))
let descent = Int(round(CTFontGetDescent(uiFont)))

var sdk = """
//! Generated proportional UI font — do not edit by hand.
//! Geist SemiBold 15px, printable ASCII, 8bpp alpha, baseline metrics.
//! Regenerate: swift apps/solitaire/genglyphs.swift assets/geist-semibold.ttf \\
//!     apps/solitaire/src/glyphs.rs apps/sdk/src/uifont.rs

/// One glyph: bitmap `data` (w*h coverage bytes) placed at
/// (pen + ox, baseline - oy); the pen then advances by `adv`.
pub struct UiGlyph {
    pub w: i32,
    pub h: i32,
    pub ox: i32,
    pub oy: i32,
    pub adv: i32,
    pub data: &'static [u8],
}

pub const ASCENT: i32 = \(ascent);
pub const LINE_H: i32 = \(ascent + descent);

"""

var names: [String] = []
for code in 32...126 {
    let ch = String(UnicodeScalar(code)!)
    let l = line(ch, uiFont)
    let bounds = CTLineGetImageBounds(l, nil)
    let adv = Int(round(CTLineGetTypographicBounds(l, nil, nil, nil)))
    let name = "G\(code)"
    names.append("&" + name)
    if bounds.width <= 0 {
        sdk += "static \(name): UiGlyph = UiGlyph { w: 0, h: 0, ox: 0, oy: 0, adv: \(adv), data: &[] };\n"
        continue
    }
    let (w, h, buf) = rasterize(l, bounds)
    let ox = Int(floor(bounds.minX)) - pad
    let oy = Int(ceil(bounds.maxY)) + pad
    sdk += "static \(name): UiGlyph = UiGlyph { w: \(w), h: \(h), ox: \(ox), oy: \(oy), adv: \(adv), data: &[\(bytes(buf))] };\n"
}

sdk += "\n/// Printable ASCII 32..=126.\npub static GLYPHS: [&UiGlyph; 95] = [" + names.joined(separator: ", ") + "];\n"

try! sdk.write(toFile: sdkOut, atomically: true, encoding: .utf8)
print("wrote \(sdkOut)")
