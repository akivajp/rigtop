# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.26.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.26.1/rigtop-v0.26.1-aarch64-apple-darwin.tar.gz"
    sha256 "3341be39b2091221b2258df926798d3d305b720b9f8bf690330be413bb85d220"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.26.1/rigtop-v0.26.1-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ddc1dd9e8af37c236d7665f7404d916b08475fb279d80ae3617ae4c93a1f6e7f"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.26.1/rigtop-v0.26.1-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "4f527f170795b3253133822b5498dccdf464f15668e9871aa2270cc571acb9be"
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
