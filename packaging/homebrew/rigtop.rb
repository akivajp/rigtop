# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.25.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.25.0/rigtop-v0.25.0-aarch64-apple-darwin.tar.gz"
    sha256 "a765133d08ca4b8a8f87d9c0d9816c124497eaad2ff96e9b9044cf6c222ef7ae"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.25.0/rigtop-v0.25.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "9c0be38786b62e4ce27ed7aef67acdd23c1b1e8ba99869bc63d71c34a58c229d"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.25.0/rigtop-v0.25.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "b4668e7a7edfab11f9f5a1474baf17b901be02890db9464e1cb063d3ee230db7"
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
