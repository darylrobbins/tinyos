// Generate the SDK monospace font atlas (GeistMono) as 8bpp alpha masks.
// Unlike genglyphs.swift (which renders "Geist SemiBold" by name), this loads
// the font DIRECTLY FROM THE TTF FILE, so the atlas is genuinely GeistMono and
// every glyph shares one advance — required for fixed-advance cell rendering.
// Usage: swift apps/sdk/genmono.swift assets/geistmono.ttf apps/sdk/src/monofont.rs
import Foundation
import CoreText
import CoreGraphics

let ttfPath = CommandLine.arguments[1]
let out = CommandLine.arguments[2]

guard let data = NSData(contentsOfFile: ttfPath),
      let provider = CGDataProvider(data: data),
      let cgFont = CGFont(provider) else { fatalError("cannot load font \(ttfPath)") }
let px: CGFloat = 15
let font = CTFontCreateWithGraphicsFont(cgFont, px, nil, nil)

let pad = 2
let ascent = Int(round(CTFontGetAscent(font)))
let descent = Int(round(CTFontGetDescent(font)))

func line(_ text: String) -> CTLine {
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

func bytes(_ d: [UInt8]) -> String { d.map { String($0) }.joined(separator: ",") }

// Monospace advance: constant across glyphs; measure a full-width one.
let advance = Int(round(CTLineGetTypographicBounds(line("M"), nil, nil, nil)))

var sdk = """
//! Generated monospace font — do not edit the glyph data by hand.
//! GeistMono 15px, printable ASCII, 8bpp alpha, baseline metrics.
//! Regenerate: swift apps/sdk/genmono.swift assets/geistmono.ttf apps/sdk/src/monofont.rs

pub use crate::uifont::UiGlyph;

pub const ASCENT: i32 = \(ascent);
pub const LINE_H: i32 = \(ascent + descent);
pub const ADVANCE: i32 = \(advance); // fixed monospace cell width

"""

var names: [String] = []
for code in 32...126 {
    let ch = String(UnicodeScalar(code)!)
    let l = line(ch)
    let bounds = CTLineGetImageBounds(l, nil)
    let name = "G\(code)"
    names.append("&" + name)
    if bounds.width <= 0 {
        sdk += "static \(name): UiGlyph = UiGlyph { w: 0, h: 0, ox: 0, oy: 0, adv: \(advance), data: &[] };\n"
        continue
    }
    let (w, h, buf) = rasterize(l, bounds)
    let ox = Int(floor(bounds.minX)) - pad
    let oy = Int(ceil(bounds.maxY)) + pad
    sdk += "static \(name): UiGlyph = UiGlyph { w: \(w), h: \(h), ox: \(ox), oy: \(oy), adv: \(advance), data: &[\(bytes(buf))] };\n"
}
sdk += "\n/// Printable ASCII 32..=126.\npub static GLYPHS: [&UiGlyph; 95] = [" + names.joined(separator: ", ") + "];\n"
try! sdk.write(toFile: out, atomically: true, encoding: .utf8)
print("wrote \(out) — GeistMono \(px)px, advance \(advance), ascent \(ascent), line_h \(ascent + descent)")
