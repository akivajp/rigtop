# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.21.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.21.0/rigtop-v0.21.0-aarch64-apple-darwin.tar.gz"
    sha256 "5044b9a2ac98743edd5e5084c3d531ca89b1aa9057e13b366e0b161d5c036145"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.21.0/rigtop-v0.21.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "f747ccd1443fb75e4b3e402ec57d0043e09926f1e02594c29efd41931ce93804"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.21.0/rigtop-v0.21.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "594f122582acbd63a2614d5d0d80b2b7ffab46d6046e43a7d77fb3e9e0ba0445"
    end
  end

  def install
    bin.install "rigtop"
    # The Linux aarch64 build has the terminal UI only.
    bin.install "rigtop-gui" if File.exist?("rigtop-gui")
  end

  def caveats
    <<~EOS
      Update rigtop with `brew upgrade rigtop`; its own update button
      only points there for a Homebrew install.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rigtop --version")
  end
end
