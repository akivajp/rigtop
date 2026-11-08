# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.31.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.31.0/rigtop-v0.31.0-aarch64-apple-darwin.tar.gz"
    sha256 "ecd39359b3075e9b88b1018e1779f5f14479150ab3d0edd3e2bf9b3f8f6f23f8"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.31.0/rigtop-v0.31.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d37e59ee19adde6285fb34fa544fef73cb9e638663eaf82809ff538caba70d6a"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.31.0/rigtop-v0.31.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "a2b35eb9199d6cafba694004f9a18cebb265fb8538c2486e67f390f3aa7af1a0"
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
