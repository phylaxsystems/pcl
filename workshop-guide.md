# Assertions Workshop Setup Guide

## Prerequisites

Before starting the workshop, ensure you have:

- `git`

## Setup Steps

1. Clone the repository:

```bash
git clone https://github.com/phylaxsystems/pcl.git
cd pcl
```

2. Switch to the workshop branch:

```bash
git checkout assertions-workshop
```

3. Download the required binaries:

- Find the binaries for your platform: <https://github.com/phylaxsystems/pcl/releases>
- ARM binaries might be broken, so use this link: <https://drive.google.com/drive/folders/1bdConErBMhq6UXrLKhmvHrvSQdwDa-B_?usp=sharing>

4. Make the binary executable and move it to your PATH:

```bash
# For Linux/MacOS:
chmod +x pcl-<your-platform>
sudo mv pcl-<your-platform> /usr/local/bin/pcl
```

and do the same for phorge:

```bash
chmod +x phorge-<your-platform>
sudo mv phorge-<your-platform> /usr/local/bin/phorge
```

5. Verify the installation:

```bash
pcl --version
```

## Workshop Structure

The workshop materials are located in the `testdata/mock-protocol` directory. This includes:

- Sample contracts in `src/`
- Assertion examples in `assertions/src/`
- Test files in `assertions/test/`
