import SwiftUI

struct ContentView: View {
    let version: String

    var body: some View {
        VStack(spacing: 12) {
            Text("crt")
                .font(.largeTitle)
            Text(version)
                .foregroundStyle(.secondary)
        }
        .frame(minWidth: 480, minHeight: 320)
        .padding()
    }
}
