// Removes the white rim around an app icon and fits it to the macOS icon grid: the dark body is
// clipped to a rounded square with a transparent outside and scaled to 824/1024 of the canvas
// (100/1024 margin, Apple's template), so it matches other Dock icons in size.
// Run: swiftc -O -o /tmp/deborder scripts/icon_deborder.swift && /tmp/deborder in.png out.png 3
import AppKit
import CoreGraphics
// usage: deborder <in> <out> [inset]  — inset (px) trims the antialiased rim off the body.
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
// Apple's macOS icon grid on a 1024 canvas: an 824 px rounded square with a 100 px margin. Scale
// the body onto it. The corners follow the body's own (scaled) radius, but never tighter than
// the template's ~185 px: a tighter clip would let the source's rim show along the curve.
let side = Double(w) * 824 / 1024
let target = CGRect(x: (Double(w) - side) / 2, y: (Double(h) - side) / 2, width: side, height: side)
let sx = target.width / body.width, sy = target.height / body.height
let radius = max(Double(w) * 185.4 / 1024, r * max(sx, sy) + 4)
let out = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: cs,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
out.interpolationQuality = .high
out.addPath(CGPath(roundedRect: target, cornerWidth: radius, cornerHeight: radius, transform: nil))
out.clip()
out.draw(cg, in: CGRect(x: target.minX - body.minX * sx, y: target.minY - body.minY * sy,
                        width: Double(w) * sx, height: Double(h) * sy))
let rep = NSBitmapImageRep(cgImage: out.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
