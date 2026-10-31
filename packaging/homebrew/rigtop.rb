# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.22.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.22.0/rigtop-v0.22.0-aarch64-apple-darwin.tar.gz"
    sha256 "cd622e1188359628c5867a4846843db2844198322f7f0ab6aec7f85f1d2c6b13"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.22.0/rigtop-v0.22.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "2ccef12e35ff790d6ffccc8f31c24cdd3d0d7574edeb7b8fe11a494adf23d2fd"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.22.0/rigtop-v0.22.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "2dd1ccb3cbb3c7fbece67eb3271b2d7104bf058b9c0f43e8f746b0736e7020c0"
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
