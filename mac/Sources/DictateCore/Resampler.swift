import AVFoundation

public enum Resampler {
    /// Parakeet's rate. Both engines take 16 kHz mono samples, and it is the smallest upload to the PC.
    public static let modelRate: Double = 16_000

    public static func resample(_ samples: [Float], from rate: Double, to target: Double = modelRate) throws -> [Float] {
        guard rate != target, !samples.isEmpty else {
            return samples
        }
        guard let inFormat = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1),
            let outFormat = AVAudioFormat(standardFormatWithSampleRate: target, channels: 1),
            let converter = AVAudioConverter(from: inFormat, to: outFormat),
            let input = AVAudioPCMBuffer(pcmFormat: inFormat, frameCapacity: AVAudioFrameCount(samples.count))
        else {
            throw ResamplerError("no converter from \(rate) Hz to \(target) Hz")
        }
        samples.withUnsafeBufferPointer { input.floatChannelData![0].update(from: $0.baseAddress!, count: samples.count) }
        input.frameLength = AVAudioFrameCount(samples.count)

        var output: [Float] = []
        output.reserveCapacity(Int(Double(samples.count) * target / rate) + 1)
        var fed = false
        while true {
            guard let chunk = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: 16_384) else {
                throw ResamplerError("no output buffer")
            }
            var error: NSError?
            let status = converter.convert(to: chunk, error: &error) { _, inputStatus in
                if fed {
                    inputStatus.pointee = .endOfStream
                    return nil
                }
                fed = true
                inputStatus.pointee = .haveData
                return input
            }
            if status == .error {
                throw ResamplerError(error?.localizedDescription ?? "conversion failed")
            }
            output.append(contentsOf: UnsafeBufferPointer(start: chunk.floatChannelData![0], count: Int(chunk.frameLength)))
            if status == .endOfStream || (status == .inputRanDry && chunk.frameLength == 0) {
                return output
            }
        }
    }
}

public struct ResamplerError: Error, CustomStringConvertible {
    public let description: String

    init(_ description: String) {
        self.description = description
    }
}
