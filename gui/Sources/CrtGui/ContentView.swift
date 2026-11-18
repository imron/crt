import SwiftUI

struct ContentView: View {
    let snapshot: GuiSnapshot

    var body: some View {
        VSplitView {
            metadata
                .padding()
            fileList
        }
        .frame(minWidth: 560, minHeight: 400)
    }

    private var metadata: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("crt")
                .font(.largeTitle)
            Text(snapshot.version)
                .foregroundStyle(.secondary)
            Divider()
            labeled("Repository", snapshot.repoName)
            labeled("Worktree", snapshot.worktree)
            labeled("HEAD", snapshot.headRef)
            labeled("Merge base", snapshot.mergeBase)
            labeled("Server", snapshot.status)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var fileList: some View {
        List(snapshot.files) { file in
            HStack {
                Text(file.path)
                    .font(.system(.body, design: .monospaced))
                Spacer()
                Text(file.status)
                    .foregroundStyle(.secondary)
                Text(file.kind)
                    .foregroundStyle(.tertiary)
            }
        }
        .listStyle(.inset)
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
