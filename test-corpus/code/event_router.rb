# frozen_string_literal: true

# event_router.rb — Atlas event routing layer.
#
# Routes inbound IngestEvents to the correct handler based on their source
# and payload schema. Uses a mixin-based plugin system so new handlers can
# be added without modifying this file.

require "json"
require "logger"
require "securerandom"

# Type alias expressed as a constant.
# WHY: Ruby doesn't have first-class type aliases, so we document the
# expected shape here as a frozen hash template for team reference.
EVENT_SCHEMA_VERSION = "2.0"

module Atlas
  module Routing
    # Mixin that any handler class must include to register itself with the router.
    module Handler
      def self.included(base)
        base.extend(ClassMethods)
      end

      module ClassMethods
        # Register a source pattern this handler should respond to.
        def handles(source_pattern)
          EventRouter.register(source_pattern, self)
        end
      end

      # Process one event. Must be implemented by the including class.
      #
      # @param event [Hash] validated IngestEvent hash
      # @return [Hash] result with :event_id, :success, :latency_ms
      def handle(_event)
        raise NotImplementedError, "#{self.class}#handle not implemented"
      end
    end

    # Routes events to registered handlers.
    class EventRouter
      @registry = {}

      class << self
        attr_reader :registry

        # Register *handler_class* for all events whose source matches *pattern*.
        def register(pattern, handler_class)
          @registry[pattern] = handler_class
        end

        # Look up the best-matching handler for a source string.
        def resolve(source)
          _pattern, klass = @registry.find { |pat, _| source.match?(pat) }
          klass
        end
      end

      def initialize
        @logger = Logger.new($stdout, progname: "atlas.event_router")
      end

      # Route *event* to the appropriate handler and return the result.
      #
      # NOTE: if no handler matches, the event is forwarded to the
      # fallback dead-letter handler rather than raising.
      def route(event)
        source = event.fetch("source", "unknown")
        handler_class = self.class.resolve(source) || DeadLetterHandler

        handler = handler_class.new
        start = Process.clock_gettime(Process::CLOCK_MONOTONIC)

        begin
          result = handler.handle(event)
          result[:latency_ms] = ((Process.clock_gettime(Process::CLOCK_MONOTONIC) - start) * 1000).round(2)
          result
        rescue StandardError => e
          @logger.error("handler #{handler_class} raised: #{e.message}")
          { event_id: event["id"], success: false, error: e.message, latency_ms: 0 }
        end
      end

      # Batch-route an array of events, returning results in the same order.
      def route_batch(events)
        events.map { |e| route(e) }
      end

      # method_missing provides convenience routing methods like #route_kafka, #route_http
      def method_missing(name, *args, **kwargs, &block)
        if name.to_s.start_with?("route_")
          source_hint = name.to_s.delete_prefix("route_")
          event = args.first || {}
          route(event.merge("source_hint" => source_hint))
        else
          super
        end
      end

      def respond_to_missing?(name, include_private = false)
        name.to_s.start_with?("route_") || super
      end
    end

    # Fallback handler: logs and stores events that matched no registered handler.
    class DeadLetterHandler
      include Handler

      # HACK: writing to STDOUT is fine in development; production routes to
      # the dead-letter Kafka topic (see config/kafka.yml, dlq_topic key).
      def handle(event)
        $stdout.puts JSON.generate({ dlq: true, event: event })
        { event_id: event["id"], success: false, reason: "no_handler" }
      end
    end
  end
end
