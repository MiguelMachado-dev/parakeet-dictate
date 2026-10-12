import Foundation
import Testing

@testable import DictateCore

// Compares the Swift styles with frames rendered by the Python ones (mac/scripts/overlay_reference.py).
// Runs only when OVERLAY_REFERENCE_DIR points at those frames.

private let referenceDir = ProcessInfo.processInfo.environment["OVERLAY_REFERENCE_DIR"].map(URL.init(fileURLWithPath:))

private struct Entry: Decodable {
    let style: String
    let file: String
    let phase: String
    let phase_elapsed: Double
    let presence: Double
    let hands_free: Bool
    let recording_elapsed: Double
    let levels: [Float]
    let spectrum: [Float]
    let scale: Double
    let width: Int
    let height: Int

    var frame: OverlayFrame {
        OverlayFrame(
            phase: Phase(rawValue: phase)!, phaseElapsed: phase_elapsed, presence: presence, handsFree: hands_free,
            recordingElapsed: recording_elapsed, levels: levels, spectrum: spectrum, scale: scale)
    }
}

struct Difference: CustomStringConvertible {
    var worst = 0
    var off = 0
    var pixels = 0

    var description: String { "worst \(worst), \(off) of \(pixels) pixels off by more than 3" }
}

/// Channel differences after premultiplying the reference, which is straight alpha.
func compare(_ image: RGBAImage, toStraight reference: [UInt8]) -> Difference {
    var result = Difference(pixels: image.width * image.height)
    for i in stride(from: 0, to: reference.count, by: 4) {
        let alpha = Int(reference[i + 3])
        var worst = abs(Int(image.pixels[i + 3]) - alpha)
        for c in 0..<3 {
            let expected = (Int(reference[i + c]) * alpha + 127) / 255
            worst = max(worst, abs(Int(image.pixels[i + c]) - expected))
        }
        result.worst = max(result.worst, worst)
        if worst > 3 {
            result.off += 1
        }
    }
    return result
}

@Test(.enabled(if: referenceDir != nil), arguments: OverlayStyle.allCases)
func overlayReferenceFramesMatch(style: OverlayStyle) throws {
    let dir = try #require(referenceDir)
    let entries = try JSONDecoder().decode([Entry].self, from: Data(contentsOf: dir.appendingPathComponent("frames.json")))
        .filter { $0.style == style.rawValue }
    try #require(!entries.isEmpty)
    var failures: [String] = []
    for entry in entries {
        let image = style.render(entry.frame)
        #expect(image.width == entry.width && image.height == entry.height, "\(entry.file) size")
        let reference = [UInt8](try Data(contentsOf: dir.appendingPathComponent(entry.file)))
        let difference = compare(image, toStraight: reference)
        // Float32 NumPy and Swift round differently in places, and Pillow rounds when it unpremultiplies.
        if difference.off * 1000 > difference.pixels || difference.worst > 24 {
            failures.append("\(entry.file) \(entry.phase) t=\(entry.phase_elapsed) p=\(entry.presence): \(difference)")
        }
    }
    #expect(failures.isEmpty, "\(failures.count) of \(entries.count) frames differ:\n\(failures.joined(separator: "\n"))")
}
