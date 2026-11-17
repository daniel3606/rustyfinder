import AppKit
import ImageIO

/// Metadata for Multi-Rename's Add Date (Metadata) and Add Metadata rules:
/// ImageIO for photos (it reads EXIF even where Spotlight has not indexed),
/// Spotlight for audio, video and documents.
enum RenameMetadataReader {
    /// Keys the Add Metadata rule offers, in menu order.
    static let keys = [
        "Dimensions", "Width", "Height", "Camera", "Lens", "Artist", "Title", "Album", "Track", "Year", "Duration",
        "Pages",
    ]

    /// Read what is known about a local file. Slow-ish: call off the main thread.
    static func read(path: String) -> RenameMetadata {
        var values: [String: String] = [:]
        var date: Int64?
        let url = URL(fileURLWithPath: path)

        if let source = CGImageSourceCreateWithURL(url as CFURL, nil),
           let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        {
            if let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int {
                values["Width"] = "\(abs(w))"
                values["Height"] = "\(abs(h))"
                values["Dimensions"] = "\(abs(w))x\(abs(h))"
            }
            let tiff = props[kCGImagePropertyTIFFDictionary] as? [CFString: Any]
            let exif = props[kCGImagePropertyExifDictionary] as? [CFString: Any]
            if let model = tiff?[kCGImagePropertyTIFFModel] as? String { values["Camera"] = clean(model) }
            if let lens = exif?[kCGImagePropertyExifLensModel] as? String { values["Lens"] = clean(lens) }
            if let taken = (exif?[kCGImagePropertyExifDateTimeOriginal] ?? exif?[kCGImagePropertyExifDateTimeDigitized]) as? String {
                date = exifDate(taken)
            }
        }

        if let item = NSMetadataItem(url: url) {
            func text(_ key: CFString) -> String? {
                switch item.value(forAttribute: key as String) {
                case let list as [Any]: return list.map { "\($0)" }.joined(separator: ", ")
                case let number as NSNumber: return number.stringValue
                case let string as String: return string
                default: return nil
                }
            }
            let pairs: [(String, CFString)] = [
                ("Artist", kMDItemAuthors), ("Title", kMDItemTitle), ("Album", kMDItemAlbum),
                ("Track", kMDItemAudioTrackNumber), ("Year", kMDItemRecordingYear), ("Pages", kMDItemNumberOfPages),
                ("Camera", kMDItemAcquisitionModel), ("Lens", kMDItemLensModel),
            ]
            for (name, key) in pairs where values[name] == nil {
                if let value = text(key).map(clean), !value.isEmpty { values[name] = value }
            }
            if values["Duration"] == nil, let seconds = item.value(forAttribute: kMDItemDurationSeconds as String) as? NSNumber {
                let s = Int(seconds.doubleValue.rounded())
                values["Duration"] = s >= 3600
                    ? String(format: "%d.%02d.%02d", s / 3600, s / 60 % 60, s % 60)
                    : String(format: "%d.%02d", s / 60, s % 60)
            }
            // Spotlight falls back to the file's own creation date for
            // files with no content date of their own: that is no metadata.
            if date == nil, let created = item.value(forAttribute: kMDItemContentCreationDate as String) as? Date,
               let onDisk = (try? url.resourceValues(forKeys: [.creationDateKey]))?.creationDate,
               abs(created.timeIntervalSince(onDisk)) > 1
            {
                date = Int64(created.timeIntervalSince1970 * 1000)
            }
        }
        return RenameMetadata(dateMs: date, values: values)
    }

    /// Values go into a file name: no slashes, colons or line breaks.
    private static func clean(_ text: String) -> String {
        text.components(separatedBy: CharacterSet(charactersIn: "/:\n\r\t")).joined(separator: " ")
            .trimmingCharacters(in: .whitespaces)
    }

    /// EXIF writes "2024:12:30 14:05:09" in the camera's local time.
    private static func exifDate(_ text: String) -> Int64? {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy:MM:dd HH:mm:ss"
        return formatter.date(from: text).map { Int64($0.timeIntervalSince1970 * 1000) }
    }
}
