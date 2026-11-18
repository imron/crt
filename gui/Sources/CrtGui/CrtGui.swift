import AppKit
import Foundation
import SwiftUI

private var retainedDelegate: AppDelegate?
private var retainedStore: GuiStore?

@_silgen_name("crt_gui_submit_action")
func crt_gui_submit_action(_ data: UnsafePointer<UInt8>, _ size: Int)

@_cdecl("crt_gui_run")
public func crtGuiRun(
    _ data: UnsafePointer<UInt8>,
    _ size: Int,
    _ icon: UnsafePointer<UInt8>,
    _ iconSize: Int
) {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    if let image = nsImage(from: icon, size: iconSize) {
        app.applicationIconImage = squarePaddedIcon(from: image)
    }
    let snapshot = decodeSnapshot(data, size)
    let store = GuiStore(snapshot: snapshot)
    retainedStore = store

    let delegate = AppDelegate(store: store)
    retainedDelegate = delegate
    app.delegate = delegate
    app.mainMenu = makeMainMenu()
    app.run()
}

@_cdecl("crt_gui_apply_snapshot")
public func crtGuiApplySnapshot(_ data: UnsafePointer<UInt8>, _ size: Int) {
    let snapshot = decodeSnapshot(data, size)
    DispatchQueue.main.async {
        retainedStore?.snapshot = snapshot
    }
}

final class GuiStore: ObservableObject {
    @Published var snapshot: GuiSnapshot

    init(snapshot: GuiSnapshot) {
        self.snapshot = snapshot
    }

    func selectFile(_ path: String) {
        guard path != snapshot.selectedPath else {
            return
        }
        let action = ActionEnvelope(data: SelectFileAction(path: path))
        guard let json = try? snapshotEncoder.encode(action) else {
            return
        }
        json.withUnsafeBytes { raw in
            guard let ptr = raw.bindMemory(to: UInt8.self).baseAddress else {
                return
            }
            crt_gui_submit_action(ptr, json.count)
        }
    }
}

private let snapshotDecoder: JSONDecoder = {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return decoder
}()

private let snapshotEncoder: JSONEncoder = {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    return encoder
}()

private func nsImage(from data: UnsafePointer<UInt8>, size: Int) -> NSImage? {
    guard size > 0 else {
        return nil
    }
    let bytes = Data(bytes: data, count: size)
    return NSImage(data: bytes)
}

/// Dock tiles are square. Fit the source in a square canvas so macOS
/// cannot stretch a landscape PNG.
private func squarePaddedIcon(from image: NSImage) -> NSImage {
    let src: NSSize
    if let rep = image.representations.first, rep.pixelsWide > 0, rep.pixelsHigh > 0 {
        src = NSSize(width: CGFloat(rep.pixelsWide), height: CGFloat(rep.pixelsHigh))
    } else {
        src = image.size
    }
    let side = max(src.width, src.height)
    let fitted = NSRect(
        x: (side - src.width) / 2,
        y: (side - src.height) / 2,
        width: src.width,
        height: src.height
    )
    return NSImage(size: NSSize(width: side, height: side), flipped: false) { _ in
        NSColor.clear.setFill()
        NSRect(origin: .zero, size: NSSize(width: side, height: side)).fill()
        image.draw(in: fitted, from: .zero, operation: .sourceOver, fraction: 1)
        return true
    }
}

private func decodeSnapshot(_ data: UnsafePointer<UInt8>, _ size: Int) -> GuiSnapshot {
    guard size > 0 else {
        return .invalid("empty snapshot")
    }
    let bytes = Data(bytes: data, count: size)
    do {
        let envelope = try snapshotDecoder.decode(Envelope<GuiSnapshot>.self, from: bytes)
        if let snapshot = envelope.data {
            return snapshot
        }
        if let error = envelope.error {
            return .invalid(error.message)
        }
        return .invalid("empty envelope")
    } catch {
        return .invalid(error.localizedDescription)
    }
}

private struct Envelope<T: Decodable>: Decodable {
    let data: T?
    let error: SnapshotError?
}

private struct SnapshotError: Decodable {
    let message: String
}

private struct ActionEnvelope: Encodable {
    let data: SelectFileAction
}

private struct SelectFileAction: Encodable {
    let type = "select_file"
    let path: String
}

struct GuiFile: Decodable, Identifiable, Hashable {
    let path: String
    let kind: String
    let status: String

    var id: String { path }
}

struct GuiDiffLine: Decodable, Identifiable {
    let kind: String
    let content: String
    let oldLineno: UInt32?
    let newLineno: UInt32?

    var id: String {
        "\(kind)-\(oldLineno ?? 0)-\(newLineno ?? 0)-\(content)"
    }
}

struct GuiDiff: Decodable {
    let path: String
    let isBinary: Bool
    let lines: [GuiDiffLine]
}

struct GuiSnapshot: Decodable {
    let version: String
    let repoName: String
    let worktree: String
    let headRef: String
    let mergeBase: String
    let status: String
    let files: [GuiFile]
    let selectedPath: String?
    let diff: GuiDiff?

    static func invalid(_ message: String) -> GuiSnapshot {
        GuiSnapshot(
            version: "",
            repoName: "",
            worktree: "",
            headRef: "",
            mergeBase: "",
            status: message,
            files: [],
            selectedPath: nil,
            diff: nil
        )
    }
}

private func makeMainMenu() -> NSMenu {
    let mainMenu = NSMenu()
    let appItem = NSMenuItem()
    mainMenu.addItem(appItem)

    let appMenu = NSMenu(title: "crt")
    appMenu.addItem(
        withTitle: "Quit crt",
        action: #selector(NSApplication.terminate(_:)),
        keyEquivalent: "q"
    )
    appItem.submenu = appMenu
    return mainMenu
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let store: GuiStore
    private var window: NSWindow?

    init(store: GuiStore) {
        self.store = store
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let hosting = NSHostingView(rootView: ContentView(store: store))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.title = "Code Review Tool"
        window.contentView = hosting
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
