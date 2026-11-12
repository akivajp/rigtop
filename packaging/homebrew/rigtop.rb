# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.35.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.35.1/rigtop-v0.35.1-aarch64-apple-darwin.tar.gz"
    sha256 "022b4567704c011b09d44c5115d91a582bc8e5ee98350c88ad9082905dbeb34b"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.35.1/rigtop-v0.35.1-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "eb529b56765c04d2e63cf002542188fc925fa7556e7871b8160c4d8737cc5df6"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.35.1/rigtop-v0.35.1-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0b81fbd38a991c803cc533d33b96889c1e31fd267fb5d2d4b0429ed1d83b7faf"
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
