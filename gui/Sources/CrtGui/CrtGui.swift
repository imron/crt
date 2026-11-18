import AppKit
import Combine
import Foundation
import SwiftUI

private var retainedDelegate: AppDelegate?
private var retainedStore: GuiStore?

@_silgen_name("crt_gui_submit_action")
func crt_gui_submit_action(_ data: UnsafePointer<UInt8>, _ size: Int)

@_silgen_name("crt_gui_bootstrap")
func crt_gui_bootstrap()

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
        app.applicationIconImage = image
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
    guard let image = NSImage(data: bytes) else {
        return nil
    }
    // Keep transparent padding in the Dock tile; do not trim to opaque bounds.
    image.alignmentRect = NSRect(origin: .zero, size: image.size)
    return image
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

private let appDisplayName = "Code Review Tool"

private func makeMainMenu() -> NSMenu {
    let mainMenu = NSMenu()
    let appItem = NSMenuItem()
    appItem.title = appDisplayName
    mainMenu.addItem(appItem)

    let appMenu = NSMenu(title: appDisplayName)
    appMenu.addItem(
        withTitle: "Quit \(appDisplayName)",
        action: #selector(NSApplication.terminate(_:)),
        keyEquivalent: "q"
    )
    appItem.submenu = appMenu
    return mainMenu
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let store: GuiStore
    private var window: NSWindow?
    private var cancellables = Set<AnyCancellable>()

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
        window.contentView = hosting
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        applyWindowDocument(store.snapshot)
        DispatchQueue.main.async {
            crt_gui_bootstrap()
        }
        store.$snapshot
            .receive(on: DispatchQueue.main)
            .sink { [weak self] snapshot in
                self?.applyWindowDocument(snapshot)
            }
            .store(in: &cancellables)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func applyWindowDocument(_ snapshot: GuiSnapshot) {
        guard let window else {
            return
        }
        if let path = snapshot.selectedPath, !path.isEmpty {
            window.title = URL(fileURLWithPath: path).lastPathComponent
            let fileURL = URL(fileURLWithPath: snapshot.worktree)
                .appendingPathComponent(path)
            if FileManager.default.fileExists(atPath: fileURL.path) {
                window.representedURL = fileURL
            } else {
                window.representedURL = nil
            }
        } else {
            window.title = "Code Review Tool"
            window.representedURL = nil
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
