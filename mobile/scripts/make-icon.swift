// Draws the Anzen mark (the "A" the app shows behind its balance) as an app icon.
//
//   swift scripts/make-icon.swift assets/icon.png 1024 0.52 1            the icon, on the app's background
//   swift scripts/make-icon.swift assets/adaptive-icon.png 1024 0.36 0   Android's foreground layer, transparent
//
// Arguments: output file, size in pixels, the mark's width as a share of the icon, opaque background (1 or 0).
import AppKit
let args = CommandLine.arguments
let size = CGFloat(Double(args[2])!), scale = CGFloat(Double(args[3])!), opaque = args[4] == "1"
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0, space: space,
                        bitmapInfo: (opaque ? CGImageAlphaInfo.noneSkipLast : CGImageAlphaInfo.premultipliedLast).rawValue)!
if opaque {
    context.setFillColor(CGColor(srgbRed: 6 / 255, green: 6 / 255, blue: 7 / 255, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: size, height: size))
}
// The mark's path in its own box: 190 wide, 180 tall (as in DarkChrome's background).
let points: [(CGFloat, CGFloat)] = [(95, 0), (190, 180), (144, 180), (95, 87.2), (46, 180), (0, 180)]
let width = size * scale, unit = width / 190, height = 180 * unit
let left = (size - width) / 2, top = (size - height) / 2 - size * 0.02
let path = CGMutablePath()
for (index, point) in points.enumerated() {
    // Core Graphics counts up from the bottom.
    let at = CGPoint(x: left + point.0 * unit, y: size - (top + point.1 * unit))
    if index == 0 { path.move(to: at) } else { path.addLine(to: at) }
}
path.closeSubpath()
context.addPath(path)
context.setFillColor(CGColor(srgbRed: 0.96, green: 0.96, blue: 0.97, alpha: 1))
context.fillPath()
let image = NSBitmapImageRep(cgImage: context.makeImage()!)
try image.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[1]))
