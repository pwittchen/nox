// Removes the white rim around an app icon: clips the dark body to a rounded square with a
// transparent outside. Run: swiftc -O -o /tmp/deborder scripts/icon_deborder.swift && /tmp/deborder in.png out.png 3
import AppKit
import CoreGraphics
// usage: deborder <in> <out> [inset]  — clips the dark icon body to a rounded square with a
// transparent outside, removing the white rim.
let a = CommandLine.arguments
let inset = a.count > 3 ? Double(a[3])! : 3
let src = NSImage(contentsOfFile: a[1])!
var rect = CGRect(origin: .zero, size: src.size)
let cg = src.cgImage(forProposedRect: &rect, context: nil, hints: nil)!
let w = cg.width, h = cg.height
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: cs,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
let px = ctx.data!.assumingMemoryBound(to: UInt8.self)
func dark(_ x: Int, _ y: Int) -> Bool {
  let i = (y * w + x) * 4
  let l = 0.2126 * Double(px[i]) + 0.7152 * Double(px[i+1]) + 0.0722 * Double(px[i+2])
  return px[i+3] > 200 && l < 90
}
// Body bounds along the middle row / column (the body is dark at its edges).
let my = h / 2, mx = w / 2
var x0 = 0; while x0 < w && !dark(x0, my) { x0 += 1 }
var x1 = w - 1; while x1 > 0 && !dark(x1, my) { x1 -= 1 }
var y0 = 0; while y0 < h && !dark(mx, y0) { y0 += 1 }
var y1 = h - 1; while y1 > 0 && !dark(mx, y1) { y1 -= 1 }
// Corner radius: walk the diagonal from the top-left corner until the body starts.
var d = 0; while d < w / 2 && !dark(x0 + d, y0 + d) { d += 1 }
// Bitmap rows run top-down; CoreGraphics y runs bottom-up.
let body = CGRect(x: x0, y: h - 1 - y1, width: x1 - x0 + 1, height: y1 - y0 + 1).insetBy(dx: inset, dy: inset)
// For a circular corner of radius r the diagonal gap is r(1 - 1/√2).
let r = Double(d) / (1 - 1 / 2.0.squareRoot()) - inset
print("body", body, "diag", d, "radius", r)
let out = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: cs,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
out.addPath(CGPath(roundedRect: body, cornerWidth: r, cornerHeight: r, transform: nil))
out.clip()
out.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
let rep = NSBitmapImageRep(cgImage: out.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
