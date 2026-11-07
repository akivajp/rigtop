# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.30.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.30.0/rigtop-v0.30.0-aarch64-apple-darwin.tar.gz"
    sha256 "f8343835e184b89e19dd23de8e27680828e02dbe0fbeee2d05b07c3fbeb9a87a"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.30.0/rigtop-v0.30.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "629441ed3caa450ecae0b29311859de0c971e31f07e030ccdfb5131674fad153"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.30.0/rigtop-v0.30.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "cfd5a5589a93fa50b3f40218a8e98cd90e0abe923a2f4757dbab85637422fdbc"
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
