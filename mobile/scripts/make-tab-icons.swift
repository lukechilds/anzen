// Draws the Wallet and Vault tab icons for Android's tab bar, which takes images (iOS uses the
// system's symbols). White on clear; the tab bar tints them. Written at 1x, 2x and 3x of 24 points.
//
//   swift scripts/make-tab-icons.swift assets
import AppKit

func draw(_ name: String, into directory: String, _ body: (CGContext, CGFloat) -> Void) throws {
    for (scale, suffix) in [(1, ""), (2, "@2x"), (3, "@3x")] {
        let size = 24 * scale
        let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        // The icons are described on a 24-unit grid with its origin at the top left.
        context.translateBy(x: 0, y: CGFloat(size)); context.scaleBy(x: CGFloat(scale), y: -CGFloat(scale))
        context.setStrokeColor(.white); context.setFillColor(.white); context.setLineCap(.round); context.setLineJoin(.round)
        body(context, 24)
        let image = NSBitmapImageRep(cgImage: context.makeImage()!)
        try image.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(directory)/\(name)\(suffix).png"))
    }
}

let directory = CommandLine.arguments[1]

// Wallet: a coin with the two bars and bowls of a bitcoin sign.
try draw("tab-wallet", into: directory) { context, _ in
    context.setLineWidth(1.8)
    context.strokeEllipse(in: CGRect(x: 3, y: 3, width: 18, height: 18))
    context.setLineWidth(1.5)
    let sign = CGMutablePath()
    sign.move(to: CGPoint(x: 9.6, y: 8)); sign.addLine(to: CGPoint(x: 9.6, y: 16))
    sign.move(to: CGPoint(x: 9.6, y: 8)); sign.addLine(to: CGPoint(x: 12.4, y: 8))
    sign.addArc(center: CGPoint(x: 12.4, y: 10), radius: 2, startAngle: -.pi / 2, endAngle: .pi / 2, clockwise: false)
    sign.addLine(to: CGPoint(x: 9.6, y: 12))
    sign.move(to: CGPoint(x: 12.9, y: 12))
    sign.addArc(center: CGPoint(x: 12.9, y: 14), radius: 2, startAngle: -.pi / 2, endAngle: .pi / 2, clockwise: false)
    sign.addLine(to: CGPoint(x: 9.6, y: 16))
    sign.move(to: CGPoint(x: 11.2, y: 6.6)); sign.addLine(to: CGPoint(x: 11.2, y: 8))
    sign.move(to: CGPoint(x: 11.2, y: 16)); sign.addLine(to: CGPoint(x: 11.2, y: 17.4))
    context.addPath(sign); context.strokePath()
}

// Vault: a safe's dial, as the app draws its vault mark.
try draw("tab-vault", into: directory) { context, _ in
    context.setLineWidth(1.6)
    context.strokeEllipse(in: CGRect(x: 2.5, y: 2.5, width: 19, height: 19))
    context.setLineWidth(1.2)
    context.strokeEllipse(in: CGRect(x: 8.1, y: 8.1, width: 7.8, height: 7.8))
    context.fillEllipse(in: CGRect(x: 9.9, y: 9.9, width: 4.2, height: 4.2))
    context.setLineWidth(1)
    for tick in 0..<12 {
        let angle = CGFloat(tick) * .pi / 6
        context.move(to: CGPoint(x: 12 + sin(angle) * 6.4, y: 12 - cos(angle) * 6.4))
        context.addLine(to: CGPoint(x: 12 + sin(angle) * 7.6, y: 12 - cos(angle) * 7.6))
    }
    context.strokePath()
}
