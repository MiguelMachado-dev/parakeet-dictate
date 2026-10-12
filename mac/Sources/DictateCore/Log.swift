import Foundation
import os

/// Logs to the unified log and to ~/Library/Logs/ParakeetDictate.log. Never log transcript text, only its length.
public enum Log {
    public static let fileURL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Logs/ParakeetDictate.log")
    private static let maxBytes = 1_000_000
    private static let logger = Logger(subsystem: "dev.miguelmachado.parakeet-dictate", category: "app")
    private static let queue = DispatchQueue(label: "log")
    nonisolated(unsafe) private static var handle: FileHandle?

    /// Only the app writes the file, so tests and terminal checks leave it alone.
    public static func startFile() {
        queue.sync { handle = handle ?? openFile() }
    }

    public static func info(_ message: String) {
        logger.info("\(message, privacy: .public)")
        write("INFO", message)
    }

    public static func warning(_ message: String) {
        logger.warning("\(message, privacy: .public)")
        write("WARNING", message)
    }

    public static func error(_ message: String) {
        logger.error("\(message, privacy: .public)")
        write("ERROR", message)
    }

    private static func write(_ level: String, _ message: String) {
        let line = "\(Date().formatted(.iso8601.time(includingFractionalSeconds: true))) \(level) \(message)\n"
        queue.async { handle?.write(Data(line.utf8)) }
    }

    /// Keeps one previous file, like the Windows app's rotating log.
    private static func openFile() -> FileHandle? {
        let manager = FileManager.default
        try? manager.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let size = try? manager.attributesOfItem(atPath: fileURL.path)[.size] as? Int, size > maxBytes {
            let previous = fileURL.appendingPathExtension("1")
            try? manager.removeItem(at: previous)
            try? manager.moveItem(at: fileURL, to: previous)
        }
        if !manager.fileExists(atPath: fileURL.path) {
            manager.createFile(atPath: fileURL.path, contents: nil)
        }
        let handle = try? FileHandle(forWritingTo: fileURL)
        _ = try? handle?.seekToEnd()
        return handle
    }
}
