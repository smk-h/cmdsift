##============================================================================#
# Copyright © hk. 2022-2025. All rights reserved.
# File name  : Makefile
# Author     : 苏木
# Date       : 2026-08-02
# Description: cmdsift 测试用 C 工程，故意保留若干 error/warning
#              用于验证 cmdsift 对编译输出的分类采集能力
# 说明       : 本工程故意包含错误与警告，正常情况下无法通过链接
# 用法       : make            - 编译项目（预期失败，输出 error/warning）
#              make clean      - 清理产物
#              make help       - 显示帮助
##============================================================================#

# ---- 工具链 ----
CC       = gcc

# ---- Git 版本信息 ----
GIT_REPO_TEST = $(shell if git rev-parse --is-inside-work-tree >/dev/null 2>&1; \
					then echo "true"; \
					else echo "false"; fi ;)

ifeq ($(GIT_REPO_TEST),true)
GIT_SHA = $(shell git rev-parse --short HEAD | awk 'NR==1')
GIT_SEQ = $(shell git rev-list HEAD | wc -l)
GIT_VER_INFO = $(GIT_SHA)-$(GIT_SEQ)
GIT_SVR_PATH = $(shell git remote -v | awk 'NR==1' | sed 's/[()]//g' | sed 's/\t/ /g' |cut -d " " -f2)
endif

ifneq ($(GIT_VER_INFO),)
  GIT_CFLAGS += -DGIT_VERSION=\"$(GIT_VER_INFO)\"
else
  GIT_CFLAGS += -DGIT_VERSION=\"unknown\"
endif

ifneq ($(GIT_SVR_PATH),)
  GIT_CFLAGS += -DGIT_PATH=\"$(GIT_SVR_PATH)\"
else
  GIT_CFLAGS += -DGIT_PATH=\"unknown\"
endif

# ---- 路径定义（+= 逐行追加，便于增删）----
SRCDIRS  = src
SRCDIRS  += src/core

INCDIRS  = include

BINDIR   = bin
OBJDIR   = build/obj
DEPDIR   = build/dep

# ---- 编译/链接选项（-I 由函数自动生成）----
INCLUDES  = $(addprefix -I, $(INCDIRS))

CFLAGS    = -Wall -Wextra -std=c99 -g $(GIT_CFLAGS)
CPPFLAGS  = $(INCLUDES)
LDFLAGS   =

# ---- 源文件与目标文件 ----
SRCS      = $(foreach dir, $(SRCDIRS), $(wildcard $(dir)/*.c))
OBJS      = $(patsubst %.c, $(OBJDIR)/%.o, $(notdir $(SRCS)))
DEPS      = $(patsubst %.c, $(DEPDIR)/%.d, $(notdir $(SRCS)))
TARGET    = $(BINDIR)/cmdsift_test

# ---- 搜索路径 ----
VPATH     = $(SRCDIRS)

# ---- 伪目标 ----
.PHONY: all clean help

# ---- 默认目标 ----
all: $(TARGET)

# ---- 链接 ----
$(TARGET): $(OBJS)
	@mkdir -p $(@D)
	$(CC) $(LDFLAGS) -o $@ $^

# ---- 隐含规则（附带 -MMD 实现增量编译）----
$(OBJDIR)/%.o: %.c
	@mkdir -p $(@D) $(DEPDIR)
	$(CC) $(CFLAGS) $(CPPFLAGS) -MMD -MP -MF $(DEPDIR)/$(*F).d -c $< -o $@

# ---- 引入自动生成的依赖文件 ----
-include $(DEPS)

# ---- 清理 ----
clean:
	@rm -rf $(OBJDIR) $(DEPDIR) $(TARGET)

# ---- 帮助 ----
help:
	@echo "可用目标:"
	@echo "  make        - 编译项目（预期失败，含 error/warning）"
	@echo "  make clean  - 清理编译产物"
	@echo "  make help   - 显示本帮助"
