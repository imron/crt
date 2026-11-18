import AppKit
import Foundation
import SwiftUI

private var retainedDelegate: AppDelegate?

@_cdecl("crt_gui_run")
public func crtGuiRun(_ data: UnsafePointer<UInt8>, _ size: Int) {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let snapshot = decodeSnapshot(data, size)

    let delegate = AppDelegate(snapshot: snapshot)
    retainedDelegate = delegate
    app.delegate = delegate
    app.mainMenu = makeMainMenu()
    app.run()
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

private let snapshotDecoder: JSONDecoder = {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return decoder
}()

private struct Envelope<T: Decodable>: Decodable {
    let data: T?
    let error: SnapshotError?
}

private struct SnapshotError: Decodable {
    let message: String
}

struct GuiFile: Decodable, Identifiable {
    let path: String
    let kind: String
    let status: String

    var id: String { path }
}

struct GuiSnapshot: Decodable {
    let version: String
    let repoName: String
    let worktree: String
    let headRef: String
    let mergeBase: String
    let status: String
    let files: [GuiFile]

    static func invalid(_ message: String) -> GuiSnapshot {
        GuiSnapshot(
            version: "",
            repoName: "",
            worktree: "",
            headRef: "",
            mergeBase: "",
            status: message,
            files: []
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
    private let snapshot: GuiSnapshot
    private var window: NSWindow?

    init(snapshot: GuiSnapshot) {
        self.snapshot = snapshot
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let hosting = NSHostingView(rootView: ContentView(snapshot: snapshot))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 520),
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
