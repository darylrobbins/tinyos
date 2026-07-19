// Generate 8bpp alpha-mask glyphs for the solitaire app: card ranks from the
// repo's Geist SemiBold, suit pips + recycle arrow from Apple Symbols.
// Output: a Rust source file with (w, h, bytes) per glyph.
// Usage: swift apps/solitaire/genglyphs.swift assets/geist-semibold.ttf apps/solitaire/src/glyphs.rs
import Foundation
import CoreText
import CoreGraphics

let geistPath = CommandLine.arguments[1]
let outPath = CommandLine.arguments[2]

CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: geistPath) as CFURL, .process, nil)

func render(_ text: String, font: CTFont) -> (Int, Int, [UInt8]) {
    let attr = [
        kCTFontAttributeName: font,
        kCTForegroundColorFromContextAttributeName: kCFBooleanTrue!,
    ] as CFDictionary
    let astr = CFAttributedStringCreate(nil, text as CFString, attr)!
    let line = CTLineCreateWithAttributedString(astr)
    let bounds = CTLineGetImageBounds(line, nil)
    let pad = 2
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
        CTLineDraw(line, ctx)
    }
    return (w, h, buf)
}

let geist = CTFontCreateWithName("Geist SemiBold" as CFString, 24, nil)
let geistSmall = CTFontCreateWithName("Geist SemiBold" as CFString, 15, nil)
let symbols = CTFontCreateWithName("Apple Symbols" as CFString, 20, nil)
let symbolsBig = CTFontCreateWithName("Apple Symbols" as CFString, 52, nil)
let symbolsMid = CTFontCreateWithName("Apple Symbols" as CFString, 30, nil)

var out = """
//! Generated glyph alpha masks — do not edit by hand.
//! Ranks: Geist SemiBold 24px (small: 15px). Suits + recycle: Apple Symbols.
//! Regenerate with the genglyphs.swift script (see PR that added this file).
#![allow(dead_code)]

pub struct Glyph {
    pub w: i32,
    pub h: i32,
    pub data: &'static [u8],
}

"""

func emit(_ name: String, _ g: (Int, Int, [UInt8])) {
    let bytes = g.2.map { String($0) }.joined(separator: ",")
    out += "pub static \(name): Glyph = Glyph { w: \(g.0), h: \(g.1), data: &[\(bytes)] };\n"
}

let ranks = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"]
for (i, r) in ranks.enumerated() {
    emit("RANK_\(i)", render(r, font: geist))
    emit("RANK_SM_\(i)", render(r, font: geistSmall))
}
let suits = ["\u{2660}": "SPADE", "\u{2665}": "HEART", "\u{2666}": "DIAMOND", "\u{2663}": "CLUB"]
for (ch, name) in suits {
    emit("\(name)_SM", render(ch, font: symbols))
    emit("\(name)_BIG", render(ch, font: symbolsBig))
}
emit("RECYCLE", render("\u{27F3}", font: symbolsMid))
let digitFont = CTFontCreateWithName("Geist SemiBold" as CFString, 13, nil)
for d in 0...9 { emit("DIGIT_\(d)", render(String(d), font: digitFont)) }

out += "\npub static DIGITS: [&Glyph; 10] = [" + (0...9).map { "&DIGIT_\($0)" }.joined(separator: ", ") + "];\n"
out += "\npub static RANKS: [&Glyph; 13] = [" + (0..<13).map { "&RANK_\($0)" }.joined(separator: ", ") + "];\n"
out += "pub static RANKS_SM: [&Glyph; 13] = [" + (0..<13).map { "&RANK_SM_\($0)" }.joined(separator: ", ") + "];\n"

try! out.write(toFile: outPath, atomically: true, encoding: .utf8)
print("wrote \(outPath)")
