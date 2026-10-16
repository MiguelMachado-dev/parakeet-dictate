import AVFoundation
import AudioToolbox
import DictateCore

struct SystemAudioTapError: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}

/// This Mac's audio output as mono samples, from a Core Audio process tap over every process
/// but this app, so its own tones stay out of a meeting's Others track. The tap needs the
/// NSAudioCaptureUsageDescription consent, which macOS asks for on first use; without it the
/// tap runs and delivers silence, and no public API tells that apart from a quiet meeting, so
/// the meeting log reports the levels it heard. Process taps arrived in macOS 14.2, so on
/// 14.0 and 14.1 start() says no.
final class SystemAudioTap: @unchecked Sendable {
    /// Mono samples and the rate they run at, called on an internal queue.
    typealias Handler = (_ samples: [Float], _ rate: Double) -> Void

    private(set) var rate: Double = 0
    private var tapID: AudioObjectID = kAudioObjectUnknown
    private var aggregateID: AudioObjectID = kAudioObjectUnknown
    private var procID: AudioDeviceIOProcID?
    private let queue = DispatchQueue(label: "parakeet-dictate.system-audio")

    func start(handler: @escaping Handler) throws {
        guard procID == nil else {
            throw SystemAudioTapError("the system audio tap is already running")
        }
        guard #available(macOS 14.2, *) else {
            throw SystemAudioTapError("meeting transcription needs macOS 14.2 or later")
        }
        do {
            try startTap(handler: handler)
        } catch {
            stop()
            throw error
        }
    }

    func stop() {
        if aggregateID != kAudioObjectUnknown {
            if let procID {
                warn("stop the system audio device", AudioDeviceStop(aggregateID, procID))
                warn("destroy the system audio I/O proc", AudioDeviceDestroyIOProcID(aggregateID, procID))
                self.procID = nil
            }
            warn("destroy the system audio device", AudioHardwareDestroyAggregateDevice(aggregateID))
            aggregateID = kAudioObjectUnknown
        }
        if tapID != kAudioObjectUnknown {
            if #available(macOS 14.2, *) {
                warn("destroy the system audio tap", AudioHardwareDestroyProcessTap(tapID))
            }
            tapID = kAudioObjectUnknown
        }
        rate = 0
    }

    deinit {
        stop()
    }

    @available(macOS 14.2, *)
    private func startTap(handler: @escaping Handler) throws {
        let excluded = Self.ownProcess.map { [$0] } ?? []
        if excluded.isEmpty {
            Log.warning("could not find this app's audio process; its own sounds may reach the Others track")
        }
        let tapDescription = CATapDescription(monoGlobalTapButExcludeProcesses: excluded)
        tapDescription.uuid = UUID()
        tapDescription.muteBehavior = .unmuted
        var tap: AudioObjectID = kAudioObjectUnknown
        var status = AudioHardwareCreateProcessTap(tapDescription, &tap)
        guard status == noErr else {
            throw SystemAudioTapError("the system audio tap did not start (\(status))")
        }
        tapID = tap

        let format = try Self.tapFormat(tap)
        guard format.commonFormat == .pcmFormatFloat32, format.sampleRate > 0 else {
            throw SystemAudioTapError("the system audio is not in a format the app can read")
        }
        rate = format.sampleRate
        let sampleRate = format.sampleRate

        // A tap delivers audio only as part of an aggregate device; this one holds nothing else.
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Parakeet Dictate Meeting Tap",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceTapListKey: [
                [
                    kAudioSubTapUIDKey: tapDescription.uuid.uuidString,
                    kAudioSubTapDriftCompensationKey: true,
                ]
            ],
        ]
        var device: AudioObjectID = kAudioObjectUnknown
        status = AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &device)
        guard status == noErr else {
            throw SystemAudioTapError("the system audio device did not start (\(status))")
        }
        aggregateID = device

        var proc: AudioDeviceIOProcID?
        status = AudioDeviceCreateIOProcIDWithBlock(&proc, device, queue) { _, inputData, _, _, _ in
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: inputData, deallocator: nil)
            else {
                return
            }
            let samples = Self.monoSamples(buffer)
            if !samples.isEmpty {
                handler(samples, sampleRate)
            }
        }
        guard status == noErr, let proc else {
            throw SystemAudioTapError("the system audio I/O did not start (\(status))")
        }
        procID = proc
        status = AudioDeviceStart(device, proc)
        guard status == noErr else {
            throw SystemAudioTapError("the system audio device did not run (\(status))")
        }
    }

    private func warn(_ what: String, _ status: OSStatus) {
        if status != noErr {
            Log.warning("could not \(what): \(status)")
        }
    }

    /// This app's Core Audio process object, once it has opened an audio device of its own:
    /// the meeting starts its microphone first, so by tap time it exists. Nil meanwhile.
    private static var ownProcess: AudioObjectID? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain)
        var pid = ProcessInfo.processInfo.processIdentifier
        var object = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let status = AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, UInt32(MemoryLayout<pid_t>.size), &pid, &size, &object)
        return status == noErr && object != kAudioObjectUnknown ? object : nil
    }

    @available(macOS 14.2, *)
    private static func tapFormat(_ tap: AudioObjectID) throws -> AVAudioFormat {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain)
        var description = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        let status = AudioObjectGetPropertyData(tap, &address, 0, nil, &size, &description)
        guard status == noErr, let format = AVAudioFormat(streamDescription: &description) else {
            throw SystemAudioTapError("could not read the system audio format (\(status))")
        }
        return format
    }

    /// The buffer's channels averaged, whatever layout the tap chose.
    private static func monoSamples(_ buffer: AVAudioPCMBuffer) -> [Float] {
        guard let channelData = buffer.floatChannelData, buffer.frameLength > 0 else {
            return []
        }
        let frames = Int(buffer.frameLength)
        let channels = max(1, Int(buffer.format.channelCount))
        if buffer.format.isInterleaved {
            let interleaved = UnsafeBufferPointer(start: channelData[0], count: frames * channels)
            return (0..<frames).map { frame in
                var sum: Float = 0
                for channel in 0..<channels {
                    sum += interleaved[frame * channels + channel]
                }
                return sum / Float(channels)
            }
        }
        if channels == 1 {
            return Array(UnsafeBufferPointer(start: channelData[0], count: frames))
        }
        return (0..<frames).map { frame in
            var sum: Float = 0
            for channel in 0..<channels {
                sum += channelData[channel][frame]
            }
            return sum / Float(channels)
        }
    }
}
