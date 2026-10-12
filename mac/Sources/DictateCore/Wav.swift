import Foundation

public enum Wav {
    /// A mono WAV file of 16-bit little-endian PCM: the 44-byte RIFF header, then the samples.
    public static func encode(_ samples: [Float], rate: Int) -> Data {
        let size = UInt32(samples.count * 2)
        var data = Data(capacity: 44 + samples.count * 2)
        func u32(_ value: UInt32) { withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) } }
        func u16(_ value: UInt16) { withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) } }
        data.append(contentsOf: Array("RIFF".utf8))
        u32(36 + size)
        data.append(contentsOf: Array("WAVE".utf8))
        data.append(contentsOf: Array("fmt ".utf8))
        u32(16)
        u16(1)  // PCM
        u16(1)  // mono
        u32(UInt32(rate))
        u32(UInt32(rate * 2))  // bytes per second
        u16(2)  // bytes per frame
        u16(16)  // bits per sample
        data.append(contentsOf: Array("data".utf8))
        u32(size)
        for sample in samples {
            let clamped = max(-1, min(1, sample))
            u16(UInt16(bitPattern: Int16((clamped * 32767).rounded())))
        }
        return data
    }
}
