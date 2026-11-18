import SwiftUI

struct ContentView: View {
    let version: String
    let repoName: String
    let worktree: String
    let headRef: String
    let status: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("crt")
                .font(.largeTitle)
            Text(version)
                .foregroundStyle(.secondary)
            Divider()
            labeled("Repository", repoName)
            labeled("Worktree", worktree)
            labeled("HEAD", headRef)
            labeled("Server", status)
        }
        .frame(minWidth: 520, minHeight: 320, alignment: .leading)
        .padding()
    }

    private func labeled(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(value)
                .textSelection(.enabled)
        }
    }
}
