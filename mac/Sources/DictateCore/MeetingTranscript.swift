import Foundation

/// Which track of a meeting a segment came from: the microphone, or this Mac's audio output.
/// Two tracks are what tell the user from everyone else, with no diarization.
public enum MeetingSpeaker: String {
    case me = "Me"
    case others = "Others"

    public var label: String { rawValue }
}

/// The Markdown a meeting leaves behind. Pure formatting, so the app's writer only appends.
public enum MeetingTranscript {
    /// "01:02:03" for an offset into the meeting, always with hours so lines line up.
    public static func timestamp(_ seconds: Double) -> String {
        let total = max(0, Int(seconds.rounded(.down)))
        return String(format: "%02d:%02d:%02d", total / 3600, (total % 3600) / 60, total % 60)
    }

    public static func line(at seconds: Double, speaker: MeetingSpeaker, text: String) -> String {
        let oneLine = text.split(whereSeparator: { $0.isNewline }).joined(separator: " ")
            .trimmingCharacters(in: .whitespaces)
        return "[\(timestamp(seconds))] \(speaker.label): \(oneLine)"
    }

    public static func fileName(startedAt: Date, timeZone: TimeZone = .current) -> String {
        formatter(timeZone, "yyyy-MM-dd HH-mm").string(from: startedAt) + " Meeting.md"
    }

    public static func header(startedAt: Date, microphone: String, timeZone: TimeZone = .current) -> String {
        """
        # Meeting — \(formatter(timeZone, "yyyy-MM-dd HH:mm").string(from: startedAt))

        Microphone: \(microphone). The "Others" track is this Mac's audio output, so it holds every
        other participant together. Transcribed on this Mac by Parakeet Dictate; no audio was kept.

        ## Transcript


        """
    }

    public static func footer(
        endedAt: Date, duration: Double, droppedSegments: Int, timeZone: TimeZone = .current
    ) -> String {
        var text = "\n---\n\nEnded \(formatter(timeZone, "yyyy-MM-dd HH:mm").string(from: endedAt)) "
            + "after \(durationText(duration))."
        if droppedSegments > 0 {
            text += " \(droppedSegments) segment\(droppedSegments == 1 ? "" : "s") could not be transcribed; "
                + "the log says why."
        }
        return text + "\n"
    }

    static func durationText(_ seconds: Double) -> String {
        let total = max(0, Int(seconds.rounded()))
        if total >= 3600 {
            return "\(total / 3600) h \((total % 3600) / 60) min"
        }
        if total >= 60 {
            return "\(total / 60) min"
        }
        return "\(total) s"
    }

    private static func formatter(_ timeZone: TimeZone, _ format: String) -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = format
        return formatter
    }
}
