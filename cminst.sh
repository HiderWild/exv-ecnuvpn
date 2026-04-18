#!/bin/zsh

# 🔗 Command Installer - Create symbolic links for sh/py files in /usr/local/bin/
# The symbolic link name will be the main filename (without extension)
# Automatically adds execute permissions to the target file
# Use -c/--copy to copy files (only for extensionless files) instead of symlinking

# Parse arguments
copy_mode=false
file_path=""

while [ $# -gt 0 ]; do
    case "$1" in
        -c|--copy)
            copy_mode=true
            shift
            ;;
        -*)
            echo "❌ Error: Unknown option '$1'"
            echo "📝 Usage: $0 [-c|--copy] <file_path>"
            exit 1
            ;;
        *)
            if [ -n "$file_path" ]; then
                echo "❌ Error: Script accepts exactly one file parameter"
                echo "📝 Usage: $0 [-c|--copy] <file_path>"
                exit 1
            fi
            file_path="$1"
            shift
            ;;
    esac
done

if [ -z "$file_path" ]; then
    echo "❌ Error: No file path provided"
    echo "📝 Usage: $0 [-c|--copy] <file_path>"
    exit 1
fi

# Check if file exists
if [ ! -f "$file_path" ]; then
    echo "❌ Error: File '$file_path' does not exist"
    exit 1
fi

# Get absolute path of the file
absolute_path=$(realpath "$file_path")

# Get filename (without path)
filename=$(basename "$file_path")

# Check extension restrictions (allow .sh, .py, or no extension)
if [[ "$filename" == *.* ]]; then
    extension="${filename##*.}"
    if [ "$extension" != "sh" ] && [ "$extension" != "py" ]; then
        echo "❌ Error: File must be a .sh or .py file (or have no extension)"
        echo "🏷️  Current file extension: $extension"
        exit 1
    fi
    if $copy_mode; then
        echo "❌ Error: --copy mode only supports files without extension"
        exit 1
    fi
fi

# Get main filename (without extension)
main_name="${filename%.*}"

# Add execute permissions to the target file
echo "🔧 Adding execute permissions to the file..."
if chmod +x "$absolute_path"; then
    echo "✅ Execute permissions added successfully"
else
    echo "⚠️  Warning: Failed to add execute permissions (continuing anyway)"
fi

# Target symbolic link path
install_dir="/usr/local/bin"
link_path="$install_dir/$main_name"

# Only use sudo when /usr/local/bin is not writable by current user
use_sudo=false
if [ ! -w "$install_dir" ]; then
    use_sudo=true
fi

run_install_cmd() {
    if $use_sudo; then
        sudo "$@"
    else
        "$@"
    fi
}

# Check if target location already has a file or link
if [ -e "$link_path" ] || [ -L "$link_path" ]; then
    current_target=""
    if [ -L "$link_path" ]; then
        current_target=$(readlink "$link_path")
        echo "⚠️  Existing link detected: $link_path -> $current_target"
    else
        echo "⚠️  Existing file detected at: $link_path"
    fi

    if $copy_mode; then
        read -k 1 -r "REPLY?🤔 Replace it with a copy of $absolute_path ? (y/N): "
    else
        read -k 1 -r "REPLY?🤔 Replace it with a new link to $absolute_path ? (y/N): "
    fi
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "🚫 Operation cancelled"
        exit 0
    fi

    echo "🗑️  Removing existing entry..."
    if ! run_install_cmd rm -f "$link_path"; then
        echo "❌ Error: Failed to remove existing entry"
        exit 1
    fi
fi

if $copy_mode; then
    # Copy file
    echo "📄 Copying file: $absolute_path -> $link_path"
    if run_install_cmd cp "$absolute_path" "$link_path" && run_install_cmd chmod +x "$link_path"; then
        echo "✅ Success: File copied and made executable! 🎉"
        echo "🚀 You can now use command '$main_name' to execute this script"
        if [ "$main_name" = "exv" ]; then
            echo "🔐 Next step: run 'sudo exv service install' once to enable daily non-sudo usage"
        fi
        echo "📋 File details:"
        ls -la "$link_path"
    else
        echo "❌ Error: Failed to copy file"
        exit 1
    fi
else
    # Create symbolic link
    echo "🔗 Creating symbolic link: $link_path -> $absolute_path"
    if run_install_cmd ln -s "$absolute_path" "$link_path"; then
        echo "✅ Success: Symbolic link created! 🎉"
        echo "🚀 You can now use command '$main_name' to execute this script"
        if [ "$main_name" = "exv" ]; then
            echo "🔐 Next step: run 'sudo exv service install' once to enable daily non-sudo usage"
        fi
        echo "📋 Link details:"
        ls -la "$link_path"
    else
        echo "❌ Error: Failed to create symbolic link"
        exit 1
    fi
fi