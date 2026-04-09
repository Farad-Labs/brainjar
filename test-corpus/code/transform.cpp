/**
 * transform.cpp — Atlas C++ transformation engine.
 *
 * Provides a templated Rule<T> type and the TransformPipeline class that
 * chains rules over strongly-typed event payloads. Compiled as part of the
 * native extension used by the Python transform_engine via pybind11.
 */

#include <chrono>
#include <functional>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace atlas {
namespace transform {

/// Type alias for the function that evaluates one transformation rule.
template <typename T>
using RuleFn = std::function<T(const T&)>;

/**
 * A single named transformation rule parameterised over payload type T.
 *
 * WHY: templating the rule on T lets the compiler enforce that rules
 * and pipelines agree on the payload schema at compile time, catching
 * mismatches that would otherwise surface only at runtime.
 */
template <typename T>
class Rule {
public:
    explicit Rule(std::string name, RuleFn<T> fn, int priority = 0)
        : name_(std::move(name)), fn_(std::move(fn)), priority_(priority), enabled_(true) {}

    [[nodiscard]] const std::string& name() const noexcept { return name_; }
    [[nodiscard]] int priority() const noexcept { return priority_; }
    [[nodiscard]] bool enabled() const noexcept { return enabled_; }
    void set_enabled(bool v) noexcept { enabled_ = v; }

    /// Apply the rule to *input* and return the transformed value.
    T apply(const T& input) const { return fn_(input); }

private:
    std::string name_;
    RuleFn<T>   fn_;
    int         priority_;
    bool        enabled_;
};

/// Metrics snapshot for a single pipeline execution.
struct PipelineMetrics {
    std::string pipeline_id;
    size_t      rules_applied{0};
    int64_t     latency_us{0};
    bool        success{false};
};

/**
 * TransformPipeline chains an ordered list of Rules<T>.
 *
 * Rules are applied in ascending priority order. Any rule that throws
 * terminates the chain and records the error in the metrics.
 */
template <typename T>
class TransformPipeline {
public:
    explicit TransformPipeline(std::string id) : id_(std::move(id)) {}

    /// Add a rule to the pipeline. Rules are sorted by priority on first run.
    void add_rule(Rule<T> rule) {
        rules_.push_back(std::move(rule));
        sorted_ = false;
    }

    /**
     * Execute all enabled rules against *input* in priority order.
     *
     * NOTE: we sort lazily so callers can add rules in any order without
     * paying the sort cost on every add.
     */
    T execute(const T& input, PipelineMetrics& metrics) {
        if (!sorted_) {
            std::stable_sort(rules_.begin(), rules_.end(),
                [](const Rule<T>& a, const Rule<T>& b) {
                    return a.priority() < b.priority();
                });
            sorted_ = true;
        }

        auto start = std::chrono::steady_clock::now();
        T current = input;
        metrics.rules_applied = 0;
        metrics.pipeline_id = id_;

        for (const auto& rule : rules_) {
            if (!rule.enabled()) continue;
            try {
                current = rule.apply(current);
                ++metrics.rules_applied;
            } catch (const std::exception& ex) {
                // HACK: swallowing all exceptions here is intentional during the
                // initial rollout; per-rule error reporting is tracked in #177.
                (void)ex;
                metrics.success = false;
                metrics.latency_us = elapsed_us(start);
                return current;
            }
        }

        metrics.success    = true;
        metrics.latency_us = elapsed_us(start);
        return current;
    }

    [[nodiscard]] std::string_view id() const noexcept { return id_; }

private:
    static int64_t elapsed_us(std::chrono::steady_clock::time_point start) {
        using namespace std::chrono;
        return duration_cast<microseconds>(steady_clock::now() - start).count();
    }

    std::string    id_;
    std::vector<Rule<T>> rules_;
    bool           sorted_{true};
};

} // namespace transform
} // namespace atlas
