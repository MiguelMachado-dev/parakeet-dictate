import Foundation
import Testing

@testable import DictateCore

private let gmt = TimeZone(secondsFromGMT: 0)!

private func date(_ year: Int, _ month: Int, _ day: Int, _ hour: Int, _ minute: Int) -> Date {
    var components = DateComponents()
    components.calendar = Calendar(identifier: .gregorian)
    components.timeZone = gmt
    components.year = year
    components.month = month
    components.day = day
    components.hour = hour
    components.minute = minute
    return components.date!
}

@Test func timestampsAreHoursMinutesSeconds() {
    #expect(MeetingTranscript.timestamp(0) == "00:00:00")
    #expect(MeetingTranscript.timestamp(83.9) == "00:01:23")
    #expect(MeetingTranscript.timestamp(3723) == "01:02:03")
    #expect(MeetingTranscript.timestamp(-5) == "00:00:00")
}

@Test func aLineNamesTheSpeakerAndFlattensTheText() {
    #expect(MeetingTranscript.line(at: 83, speaker: .me, text: "  olá\nmundo ") == "[00:01:23] Me: olá mundo")
    #expect(MeetingTranscript.line(at: 0, speaker: .others, text: "hi") == "[00:00:00] Others: hi")
}

@Test func theFileNameCarriesTheStartTime() {
    #expect(MeetingTranscript.fileName(startedAt: date(2026, 10, 15, 14, 30), timeZone: gmt)
        == "2026-10-15 14-30 Meeting.md")
}

@Test func theHeaderNamesTheMeetingAndTheMicrophone() {
    let header = MeetingTranscript.header(
        startedAt: date(2026, 10, 15, 14, 30), microphone: "AirPods Pro", timeZone: gmt)
    #expect(header.hasPrefix("# Meeting — 2026-10-15 14:30\n"))
    #expect(header.contains("Microphone: AirPods Pro."))
    #expect(header.hasSuffix("## Transcript\n\n"))
}

@Test func theFooterReportsTheEndAndAnyLoss() {
    let ended = date(2026, 10, 15, 15, 2)
    let clean = MeetingTranscript.footer(endedAt: ended, duration: 1920, droppedSegments: 0, timeZone: gmt)
    #expect(clean == "\n---\n\nEnded 2026-10-15 15:02 after 32 min.\n")
    let lossy = MeetingTranscript.footer(endedAt: ended, duration: 1920, droppedSegments: 2, timeZone: gmt)
    #expect(lossy.contains("2 segments could not be transcribed"))
    let one = MeetingTranscript.footer(endedAt: ended, duration: 45, droppedSegments: 1, timeZone: gmt)
    #expect(one.contains("after 45 s. 1 segment could not be transcribed"))
}
