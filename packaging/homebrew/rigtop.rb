# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.36.2"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.36.2/rigtop-v0.36.2-aarch64-apple-darwin.tar.gz"
    sha256 "f05fd7e53d54154a80700e38ed23dd40bb445766c955ac7fcf140a665d67c44a"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.36.2/rigtop-v0.36.2-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "c4849d2d00757167d8b50c196be15fbdbd252c8a7fefa229fdbadf61d751016e"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.36.2/rigtop-v0.36.2-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "df99440ac3c1e47f049b95af0c1dd6e7f54f7f04356d70c299a8ed7e328847d9"
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
