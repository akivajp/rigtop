# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.29.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.29.0/rigtop-v0.29.0-aarch64-apple-darwin.tar.gz"
    sha256 "ff312473073dbc293d02157d4befb7d73d798f3feb9e53b969825fe9da5f0821"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.29.0/rigtop-v0.29.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d72d7e82ee4148ce34659b7c1f58253bd235523c549d2b8948644b503a1bc12a"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.29.0/rigtop-v0.29.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "3bdb21aea17dbcd8884509ddfa880e0db39345b720c3e202d0084dda4655fca2"
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
